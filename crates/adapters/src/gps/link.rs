use crate::{
    discovery::{adb_path, run},
    process_io::LineReader,
};
use inspector_domain::GpsFix;
use serde_json::{Value, json};
use std::{io::Write, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const REMOTE: &str = "/data/local/tmp/flutter-inspector-gps-v1.jar";
const INVOKE: &str = "CLASSPATH=/data/local/tmp/flutter-inspector-gps-v1.jar app_process /system/bin dev.flutterinspector.gps.Main";
const HELPER: &[u8] = include_bytes!("../../assets/gps-helper.jar");

/// One ADB process for the entire session; Android restores providers on EOF or lease expiry.
pub struct Link {
    child: Child,
    input: ChildStdin,
    output: LineReader<BufReader<ChildStdout>>,
}

async fn deploy(serial: &str) -> Result<PathBuf, String> {
    if serial.is_empty() {
        return Err("Select an Android device first".into());
    }
    let adb = adb_path();
    let file = tokio::task::spawn_blocking(|| {
        let mut file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
        file.write_all(HELPER).map_err(|e| e.to_string())?;
        Ok::<_, String>(file)
    })
    .await
    .map_err(|e| e.to_string())??;
    run(
        &adb,
        &["-s", serial, "push", &file.path().to_string_lossy(), REMOTE],
    )
    .await?;
    Ok(adb)
}

impl Link {
    pub async fn start(serial: &str) -> Result<Self, String> {
        let adb = deploy(serial).await?;
        let mut command = Command::new(adb);
        command
            .args(["-s", serial, "shell", INVOKE])
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let input = child.stdin.take().ok_or("Missing ADB stdin")?;
        let output = LineReader::new(
            BufReader::new(child.stdout.take().ok_or("Missing ADB stdout")?),
            8192,
        );
        let mut link = Self {
            child,
            input,
            output,
        };
        let ready = link.response().await?;
        if ready["ready"] != true || ready["protocol"] != 1 {
            return Err("Unsupported GPS helper response".into());
        }
        Ok(link)
    }
    async fn response(&mut self) -> Result<Value, String> {
        tokio::time::timeout(Duration::from_secs(4), async {
            let line =
                self.output.next().await.map_err(|e| e.to_string())?.ok_or(
                    "GPS helper disconnected. Reconnect USB and use Restore GPS if needed.",
                )?;
            let value: Value = serde_json::from_str(&line)
                .map_err(|_| format!("Invalid GPS helper response: {line}"))?;
            if let Some(error) = value["error"].as_str() {
                return Err(error.to_owned());
            }
            Ok(value)
        })
        .await
        .map_err(|_| {
            "GPS helper timed out. The device restores GPS after its 10-second lease expires."
                .to_owned()
        })?
    }
    async fn request(&mut self, value: Value) -> Result<Value, String> {
        let mut data = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        data.push(b'\n');
        tokio::time::timeout(Duration::from_secs(2), self.input.write_all(&data))
            .await
            .map_err(|_| "GPS write timed out".to_owned())?
            .map_err(|e| e.to_string())?;
        let response = self.response().await?;
        if response["ok"] != true {
            return Err("GPS helper did not acknowledge the command".into());
        }
        Ok(response)
    }
    pub async fn fix(&mut self, fix: GpsFix) -> Result<(), String> {
        fix.validate()?;
        let response = self.request(json!({"op":"fix", "fix":fix})).await?;
        if !response["fix"].is_object() {
            return Err("GPS helper did not acknowledge the location fix".into());
        }
        Ok(())
    }
    pub async fn stop(mut self) -> Result<(), String> {
        if self.request(json!({"op":"stop"})).await?["stopped"] != true {
            return Err("GPS restoration was not confirmed. Reconnect and use Restore GPS.".into());
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), self.child.wait()).await;
        Ok(())
    }
}

pub async fn restore(serial: &str) -> Result<(), String> {
    let adb = deploy(serial).await?;
    let text = run(
        &adb,
        &["-s", serial, "shell", &format!("{INVOKE} --restore")],
    )
    .await?;
    let value: Value = serde_json::from_str(text.trim()).map_err(|e| e.to_string())?;
    if value["ok"] != true {
        return Err(value["error"]
            .as_str()
            .unwrap_or("GPS restoration failed")
            .into());
    }
    Ok(())
}
