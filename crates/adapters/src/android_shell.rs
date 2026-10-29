//! Bounded Android shell calls for device management, always off the UI thread.
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

pub(crate) struct AndroidShell<'a> {
    pub path: &'a Path,
    pub serial: &'a str,
}
impl AndroidShell<'_> {
    pub async fn run(&self, args: &[&str], seconds: u64) -> Result<String, String> {
        if self.serial.is_empty()
            || self.serial.len() > 256
            || !self
                .serial
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._:-[]".contains(&c))
        {
            return Err("Select a valid Android device".into());
        }
        // adb joins shell arguments remotely; quote each value as shell data.
        let quoted: Vec<_> = args
            .iter()
            .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
            .collect();
        let mut command = Command::new(self.path);
        command
            .args(["-s", self.serial, "shell"])
            .args(&quoted)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|e| format!("Cannot start ADB: {e}"))?;
        let stdout = child.stdout.take().ok_or("Missing ADB stdout")?;
        let stderr = child.stderr.take().ok_or("Missing ADB stderr")?;
        let read = async |stream: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>,
                          limit: u64| {
            let mut bytes = vec![];
            stream
                .take(limit + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > limit {
                return Err("Android output exceeded the size limit".to_string());
            }
            Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
        };
        tokio::time::timeout(Duration::from_secs(seconds), async {
            let (out, err, status) = tokio::try_join!(
                read(Box::pin(stdout), 2*1024*1024), read(Box::pin(stderr), 16*1024),
                async { child.wait().await.map_err(|e| e.to_string()) }
            )?;
            if !(status.success() || args.last() == Some(&"help") && out.contains("commands:")) {
                return Err(format!("Android rejected the operation: {}", if err.is_empty() { out } else { err }.chars().take(1200).collect::<String>()));
            }
            if !err.is_empty() && (err.contains("Exception") || err.contains("Error:")) { return Err(err); }
            if out.lines().any(|line| line.starts_with("Error:") || line.starts_with("Failure") || line.starts_with("Unknown command") || line.starts_with("Unknown option")) {
                return Err(out.chars().take(1200).collect());
            }
            Ok(out)
        }).await.map_err(|_| "ADB timed out. Check the selected device and USB connection; refresh to read its current state.".to_string())?
    }
}
