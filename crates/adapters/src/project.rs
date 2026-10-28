//! Read editor launch profiles without modifying the selected project.
use inspector_application::*;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

pub fn open(path: &Path) -> Result<FlutterProject, String> {
    let root = path
        .canonicalize()
        .map_err(|e| format!("Cannot open project: {e}"))?;
    if !root.join("pubspec.yaml").is_file() {
        return Err("Choose the Flutter project folder containing pubspec.yaml".into());
    }
    let mut profiles = vec![];
    let mut notes = vec![];
    for (file, source) in [
        (".vscode/launch.json", "VS Code"),
        (".zed/debug.json", "Zed"),
    ] {
        let path = root.join(file);
        if !path.is_file() {
            continue;
        }
        let value = match read_jsonc(&path) {
            Ok(value) => value,
            Err(error) => {
                notes.push(error);
                continue;
            }
        };
        let entries = if source == "VS Code" {
            value["configurations"].as_array()
        } else {
            value.as_array()
        };
        let Some(entries) = entries else {
            notes.push(format!("{file}: unsupported configuration structure"));
            continue;
        };
        for entry in entries {
            if entry["request"].as_str().unwrap_or("launch") != "launch" {
                continue;
            }
            if !matches!(entry["type"].as_str(), Some("dart" | "flutter"))
                && !entry["adapter"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case("dart"))
            {
                continue;
            }
            let name = entry["name"]
                .as_str()
                .or_else(|| entry["label"].as_str())
                .unwrap_or("Flutter");
            match profile(entry, &root, name, source) {
                Ok(profile) => profiles.push(profile),
                Err(error) => notes.push(format!("{source} · {name}: {error}")),
            }
        }
    }
    profiles.push(LaunchProfile {
        name: "Default debug".into(),
        source: "Inspector".into(),
        cwd: root.clone(),
        program: "lib/main.dart".into(),
        tool_args: vec![],
        args: vec![],
        env: BTreeMap::new(),
        device: String::new(),
        issue: None,
    });
    let sdk = resolve_sdk(&root, &mut notes);
    Ok(FlutterProject {
        root,
        sdk,
        profiles,
        notes,
    })
}

fn profile(value: &Value, root: &Path, name: &str, source: &str) -> Result<LaunchProfile, String> {
    let cwd = expand(value["cwd"].as_str().unwrap_or("${workspaceFolder}"), root)?;
    let cwd = root.join(cwd);
    let program = expand(value["program"].as_str().unwrap_or("lib/main.dart"), root)?;
    let mut tool_args = strings(value.get("toolArgs"), root)?;
    if let Some(flavor) = value["flavor"].as_str() {
        tool_args.extend(["--flavor".into(), expand(flavor, root)?]);
    }
    let args = strings(value.get("args"), root)?;
    let mut env = BTreeMap::new();
    if let Some(file) = value["envFile"].as_str() {
        let path = root.join(expand(file, root)?);
        let text = read_bounded(&path)?;
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("envFile: expected KEY=value at line {}", index + 1))?;
            let value = value.trim();
            let value = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            env.insert(key.trim().into(), value.into());
        }
    }
    if let Some(values) = value.get("env") {
        for (key, value) in values.as_object().ok_or("env must be an object")? {
            env.insert(
                key.clone(),
                expand(value.as_str().ok_or("env values must be strings")?, root)?,
            );
        }
    }
    let mut issue = None;
    if value["flutterMode"]
        .as_str()
        .is_some_and(|mode| mode != "debug")
    {
        issue =
            Some("This profile uses release/profile mode. Choose a debug configuration.".into());
    }
    for key in [
        "preLaunchTask",
        "postDebugTask",
        "customTool",
        "vmAdditionalArgs",
    ] {
        if value.get(key).is_some_and(|v| !v.is_null()) {
            issue = Some(format!(
                "{key} is not supported by this launcher. Use a self-contained debug profile."
            ));
        }
    }
    validate_tool_args(&tool_args)?;
    Ok(LaunchProfile {
        name: name.into(),
        source: source.into(),
        cwd,
        program,
        tool_args,
        args,
        env,
        device: expand(value["deviceId"].as_str().unwrap_or_default(), root)?,
        issue,
    })
}
fn strings(value: Option<&Value>, root: &Path) -> Result<Vec<String>, String> {
    value
        .map(|v| {
            v.as_array()
                .ok_or("Arguments must be an array")?
                .iter()
                .map(|v| expand(v.as_str().ok_or("Arguments must be strings")?, root))
                .collect()
        })
        .unwrap_or_else(|| Ok(vec![]))
}
pub fn validate_tool_args(args: &[String]) -> Result<(), String> {
    for arg in args {
        let flag = arg.split('=').next().unwrap_or(arg);
        if matches!(
            flag,
            "--release"
                | "--profile"
                | "--no-debug"
                | "--no-machine"
                | "--machine"
                | "--start-paused"
                | "--no-hot"
                | "--resident"
                | "--no-resident"
                | "--web-server-debug-protocol"
                | "-d"
                | "--device-id"
                | "-t"
                | "--target"
                | "--"
        ) {
            return Err(format!(
                "{flag} conflicts with Inspector's debug controls. Set target/device using the launch fields."
            ));
        }
    }
    Ok(())
}
pub fn expand(text: &str, root: &Path) -> Result<String, String> {
    let mut text = text
        .replace("${workspaceFolder}", &root.to_string_lossy())
        .replace("${workspaceRoot}", &root.to_string_lossy())
        .replace("${ZED_WORKTREE_ROOT}", &root.to_string_lossy())
        .replace("$ZED_WORKTREE_ROOT", &root.to_string_lossy())
        .replace(
            "${workspaceFolderBasename}",
            &root.file_name().unwrap_or_default().to_string_lossy(),
        );
    while let Some(start) = text.find("${env:") {
        let end = text[start..]
            .find('}')
            .ok_or("Unclosed environment variable")?
            + start;
        let key = &text[start + 6..end];
        let value =
            std::env::var(key).map_err(|_| format!("Environment variable {key} is not set"))?;
        // Do not recursively interpret the contents of environment variables.
        if value.contains("${") {
            return Err(format!(
                "Environment variable {key} contains an unresolved placeholder"
            ));
        }
        text.replace_range(start..=end, &value);
    }
    if text.contains("${") {
        return Err("Unresolved editor variable in configuration".into());
    }
    if let Some(path) = text.strip_prefix("~/")
        && let Some(base) = directories::BaseDirs::new()
    {
        text = base.home_dir().join(path).to_string_lossy().into_owned();
    }
    Ok(text)
}
fn read_bounded(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > 1024 * 1024 {
        return Err(format!("{} exceeds 1 MiB", path.display()));
    }
    Ok(text)
}
fn read_jsonc(path: &Path) -> Result<Value, String> {
    json5::from_str(&read_bounded(path)?).map_err(|e| format!("{}: {e}", path.display()))
}
pub fn flutter_binary(sdk: &Path) -> PathBuf {
    if sdk.is_dir() {
        sdk.join("bin").join(if cfg!(windows) {
            "flutter.bat"
        } else {
            "flutter"
        })
    } else {
        sdk.to_owned()
    }
}
fn resolve_sdk(root: &Path, notes: &mut Vec<String>) -> PathBuf {
    if let Some(path) = std::env::var_os("INSPECTOR_FLUTTER") {
        return path.into();
    }
    let fvm = root.join(".fvm/flutter_sdk");
    if flutter_binary(&fvm).is_file() {
        return fvm;
    }
    let settings = root.join(".vscode/settings.json");
    if settings.is_file() {
        match read_jsonc(&settings).and_then(|v| {
            v["dart.flutterSdkPath"]
                .as_str()
                .map(|path| expand(path, root).map(|p| root.join(p)))
                .transpose()
        }) {
            Ok(Some(path)) if flutter_binary(&path).is_file() => return path,
            Ok(Some(_)) => {
                notes.push("dart.flutterSdkPath does not point to an installed Flutter SDK".into())
            }
            Err(error) => notes.push(error),
            _ => {}
        }
    }
    if let Some(path) = std::env::var_os("FLUTTER_ROOT") {
        return path.into();
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let path = dir.join(if cfg!(windows) {
                "flutter.bat"
            } else {
                "flutter"
            });
            if path.is_file() {
                return path;
            }
        }
    }
    if let Some(base) = directories::BaseDirs::new() {
        let sdk = base.home_dir().join("flutter");
        if flutter_binary(&sdk).is_file() {
            return sdk;
        }
    }
    notes.push("Flutter SDK not found. Set the SDK folder or executable before running.".into());
    "flutter".into()
}
pub async fn devices(sdk: &Path, cwd: &Path) -> Result<Vec<FlutterDevice>, String> {
    let mut command = Command::new(flutter_binary(sdk));
    command
        .args(["devices", "--machine"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot run Flutter: {e}"))?;
    let stdout = child.stdout.take().ok_or("Missing Flutter stdout")?;
    let stderr = child.stderr.take().ok_or("Missing Flutter stderr")?;
    tokio::time::timeout(Duration::from_secs(30), async {
        let read = async |stream: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>| {
            let mut data = Vec::new();
            stream
                .take(1024 * 1024 + 1)
                .read_to_end(&mut data)
                .await
                .map_err(|e| e.to_string())?;
            if data.len() > 1024 * 1024 {
                return Err("Flutter device output exceeds 1 MiB".to_owned());
            }
            Ok(String::from_utf8_lossy(&data).into_owned())
        };
        let (stdout, stderr) = tokio::try_join!(read(Box::pin(stdout)), read(Box::pin(stderr)))?;
        if !child.wait().await.map_err(|e| e.to_string())?.success() {
            return Err(stderr);
        }
        let list: Vec<Value> = serde_json::from_str(&stdout)
            .map_err(|e| format!("Invalid Flutter device list: {e}"))?;
        Ok(list
            .iter()
            .filter(|d| {
                d["isSupported"].as_bool().unwrap_or(true)
                    && d["targetPlatform"].as_str() != Some("web-javascript")
            })
            .filter_map(|d| {
                Some(FlutterDevice {
                    id: d["id"].as_str()?.into(),
                    name: d["name"].as_str()?.into(),
                    platform: d["targetPlatform"].as_str().unwrap_or_default().into(),
                })
            })
            .collect())
    })
    .await
    .map_err(|_| {
        "Flutter device discovery timed out. Check the SDK and USB connection, then retry."
            .to_owned()
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_both_editors_jsonc_and_preserves_argument_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("pubspec.yaml"), "name: test").unwrap();
        std::fs::create_dir(root.join(".vscode")).unwrap();
        std::fs::create_dir(root.join(".zed")).unwrap();
        std::fs::write(root.join(".vscode/launch.json"), r#"{ // comments
            "configurations": [{ "name": "Dev", "type": "dart", "request": "launch",
            "program": "lib/main_dev.dart", "cwd": "${workspaceFolder}",
            "toolArgs": ["--flavor", "dev", "--dart-define=LABEL=hello world"], "env": {"TEST":"x y"}, },] }"#).unwrap();
        std::fs::write(root.join(".zed/debug.json"), r#"[{"label":"Zed debug","adapter":"Dart","request":"launch","cwd":"$ZED_WORKTREE_ROOT"}]"#).unwrap();
        let project = open(root).unwrap();
        assert_eq!(project.profiles.len(), 3);
        assert_eq!(
            project.profiles[0].tool_args[2],
            "--dart-define=LABEL=hello world"
        );
        assert_eq!(project.profiles[0].env["TEST"], "x y");
        assert_eq!(project.profiles[1].cwd, root.canonicalize().unwrap());
    }
    #[test]
    fn rejects_conflicting_control_flags_and_unresolved_variables() {
        assert!(validate_tool_args(&["--release".into()]).is_err());
        assert!(validate_tool_args(&["--device-id=phone".into()]).is_err());
        assert!(expand("${command:pickSomething}", Path::new("/project")).is_err());
    }
}
