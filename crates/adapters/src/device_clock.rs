//! Explicit Android clock operations. Opening the utility only reads the device.
use inspector_application::{ClockAction, ClockResult};
use inspector_domain::{DeviceClock, local_to_epoch, validate_timezone};
use std::{
    path::Path,
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, process::Command};

const OUTPUT_LIMIT: u64 = 32 * 1024;

struct Adb<'a> {
    path: &'a Path,
    serial: &'a str,
}

impl Adb<'_> {
    async fn output(&self, args: &[&str], help: bool) -> Result<String, String> {
        let mut command = Command::new(self.path);
        command
            .args(["-s", self.serial, "shell"])
            .args(args)
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
        tokio::time::timeout(Duration::from_secs(4), async {
            let read = async |stream: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>| {
                let mut bytes = vec![];
                stream
                    .take(OUTPUT_LIMIT + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|e| e.to_string())?;
                if bytes.len() as u64 > OUTPUT_LIMIT {
                    return Err("ADB output too large".to_string());
                }
                Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
            };
            let (out, err) = tokio::try_join!(read(Box::pin(stdout)), read(Box::pin(stderr)))?;
            let status = child.wait().await.map_err(|e| e.to_string())?;
            // Android shell help commonly exits 255 even when the help is valid.
            if status.success() || (help && out.contains("commands:")) {
                return Ok(out);
            }
            let detail = if err.is_empty() { out } else { err };
            Err(format!(
                "Android rejected {}: {}",
                args.join(" "),
                detail.chars().take(700).collect::<String>()
            ))
        })
        .await
        .map_err(|_| "ADB timed out. Check the USB connection and unlock the device.".to_string())?
    }

    async fn shell(&self, args: &[&str]) -> Result<String, String> {
        self.output(args, false).await
    }

    async fn read(&self) -> Result<DeviceClock, String> {
        let (epoch, timezone, automatic_time, automatic_timezone) = tokio::try_join!(
            self.shell(&["date", "+%s"]),
            self.shell(&["getprop", "persist.sys.timezone"]),
            self.shell(&["settings", "get", "global", "auto_time"]),
            self.shell(&["settings", "get", "global", "auto_time_zone"]),
        )?;
        Ok(DeviceClock {
            epoch_millis: epoch
                .parse::<i64>()
                .ok()
                .and_then(|v| v.checked_mul(1000))
                .ok_or("Invalid device timestamp")?,
            timezone,
            automatic_time: flag(&automatic_time)?,
            automatic_timezone: flag(&automatic_timezone)?,
        })
    }

    async fn capabilities(&self) -> Result<Capabilities, String> {
        let (time, zone, alarm) = tokio::join!(
            self.output(&["cmd", "time_detector", "help"], true),
            self.output(&["cmd", "time_zone_detector", "help"], true),
            self.output(&["cmd", "alarm", "help"], true),
        );
        let time = time.unwrap_or_default();
        let zone = zone.unwrap_or_default();
        let alarm = alarm.unwrap_or_default();
        let caps = Capabilities {
            manual_time: time.contains("suggest_manual_time")
                && time.contains("--elapsed_realtime"),
            manual_zone: zone.contains("suggest_manual_time_zone"),
            auto_time: time.contains("set_auto_detection_enabled"),
            auto_zone: zone.contains("set_auto_detection_enabled"),
            alarm_time: alarm.contains("set-time TIME"),
            alarm_zone: alarm.contains("set-timezone TZ"),
        };
        Ok(caps)
    }

    async fn automatic(
        &self,
        caps: &Capabilities,
        zone: bool,
        enabled: bool,
    ) -> Result<(), String> {
        let modern = if zone { caps.auto_zone } else { caps.auto_time };
        let setting = if zone { "auto_time_zone" } else { "auto_time" };
        if modern {
            self.shell(&[
                "cmd",
                if zone {
                    "time_zone_detector"
                } else {
                    "time_detector"
                },
                "set_auto_detection_enabled",
                if enabled { "true" } else { "false" },
            ])
            .await?;
        } else {
            self.shell(&[
                "settings",
                "put",
                "global",
                setting,
                if enabled { "1" } else { "0" },
            ])
            .await?;
        }
        let actual = self.shell(&["settings", "get", "global", setting]).await?;
        if flag(&actual)? != enabled {
            return Err("Android did not accept the automatic-time setting. Check device policy or open Android settings.".into());
        }
        Ok(())
    }

    async fn timezone(&self, caps: &Capabilities, zone: &str) -> Result<(), String> {
        validate_timezone(zone)?; // Also ensures the remote shell argument contains no shell syntax.
        // Alarm commands work on stock devices where detector suggestions need
        // the privileged SUGGEST_MANUAL_TIME_AND_ZONE permission.
        if caps.alarm_zone {
            self.shell(&["cmd", "alarm", "set-timezone", zone]).await?;
        } else if caps.manual_zone {
            self.shell(&[
                "cmd",
                "time_zone_detector",
                "suggest_manual_time_zone",
                "--zone_id",
                zone,
            ])
            .await?;
        } else {
            return Err("This Android version does not expose timezone controls through ADB. Use Open Android settings.".into());
        }
        Ok(())
    }

    async fn time(&self, caps: &Capabilities, epoch_millis: i64) -> Result<(), String> {
        if epoch_millis < 0 {
            return Err("Time must be on or after the Unix epoch".into());
        }
        if caps.alarm_time {
            self.shell(&["cmd", "alarm", "set-time", &epoch_millis.to_string()])
                .await?;
        } else if caps.manual_time {
            let uptime = self.shell(&["cat", "/proc/uptime"]).await?;
            let elapsed = uptime
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite() && *n >= 0.0 && *n < 1e12)
                .ok_or("Invalid device uptime")?;
            self.shell(&[
                "cmd",
                "time_detector",
                "suggest_manual_time",
                "--elapsed_realtime",
                &((elapsed * 1000.0) as i64).to_string(),
                "--unix_epoch_time",
                &epoch_millis.to_string(),
            ])
            .await?;
        } else {
            return Err("This Android version does not expose clock controls through ADB. Use Open Android settings.".into());
        }
        Ok(())
    }
}

struct Capabilities {
    manual_time: bool,
    manual_zone: bool,
    auto_time: bool,
    auto_zone: bool,
    alarm_time: bool,
    alarm_zone: bool,
}

fn flag(value: &str) -> Result<bool, String> {
    match value {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err("Android did not return its automatic-time settings".into()),
    }
}

enum Change {
    Time(i64),
    Zone(String),
    Automatic { time: bool, zone: bool },
}

/// The caller serializes operations. No ambient/default ADB device is ever used.
pub async fn execute(
    path: &Path,
    serial: &str,
    action: ClockAction,
) -> Result<ClockResult, String> {
    if serial.is_empty()
        || serial.len() > 200
        || serial.starts_with('-')
        || !serial
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
    {
        return Err("Select a valid connected Android device.".into());
    }
    // Reject malformed values before any device operation or automatic-setting write.
    match &action {
        ClockAction::SetTimezone(zone) => validate_timezone(zone)?,
        ClockAction::SetDateTime {
            date,
            time,
            timezone,
        } => {
            local_to_epoch(date, time, timezone)?;
        }
        _ => {}
    }
    let adb = Adb { path, serial };
    if matches!(action, ClockAction::OpenSettings) {
        let output = adb
            .shell(&["am", "start", "-a", "android.settings.DATE_SETTINGS"])
            .await?;
        if output.contains("Error:") {
            return Err(format!(
                "Android could not open date/time settings: {output}"
            ));
        }
        return Ok(ClockResult {
            clock: adb.read().await?,
            message: "Date & time settings opened on the device.".into(),
        });
    }
    let before = adb.read().await?;
    let before_at = Instant::now();
    if matches!(action, ClockAction::Read) {
        return Ok(ClockResult {
            clock: before,
            message: "Device time read. No settings changed.".into(),
        });
    }
    let caps = adb.capabilities().await?;
    let change = match action {
        ClockAction::SetTimezone(zone) => Change::Zone(zone),
        ClockAction::SetDateTime {
            date,
            time,
            timezone,
        } => {
            if timezone != before.timezone {
                return Err(
                    "Device timezone changed. Refresh the device before applying the time.".into(),
                );
            }
            Change::Time(local_to_epoch(&date, &time, &timezone)?)
        }
        ClockAction::SyncComputer => Change::Time(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_millis() as i64,
        ),
        ClockAction::SetAutomatic { time, timezone } => Change::Automatic {
            time,
            zone: timezone,
        },
        _ => unreachable!(),
    };
    // Discover unsupported controls before disabling automatic detection.
    if matches!(change, Change::Time(_)) && !(caps.manual_time || caps.alarm_time)
        || matches!(change, Change::Zone(_)) && !(caps.manual_zone || caps.alarm_zone)
    {
        return Err(
            "Clock changes are unavailable on this Android device. Use Open Android settings."
                .into(),
        );
    }
    let mut value_attempted = false;
    let result = async {
        match &change {
            Change::Time(epoch) => {
                adb.automatic(&caps, false, false).await?;
                value_attempted = true;
                adb.time(&caps, *epoch).await?;
            }
            Change::Zone(zone) => {
                adb.automatic(&caps, true, false).await?;
                value_attempted = true;
                adb.timezone(&caps, zone).await?;
            }
            Change::Automatic { time, zone } => {
                adb.automatic(&caps, false, *time).await?;
                adb.automatic(&caps, true, *zone).await?;
            }
        }
        let clock = adb.read().await?;
        match &change {
            Change::Time(epoch) if clock.automatic_time || clock.epoch_millis.abs_diff(*epoch) > 5000 => {
                return Err("Android did not apply the requested time. The device may restrict manual time changes.".to_string());
            }
            Change::Zone(zone) if clock.automatic_timezone || clock.timezone != *zone => {
                return Err("Android did not apply the requested timezone. The device may restrict manual timezone changes.".to_string());
            }
            _ => {}
        }
        Ok(ClockResult { clock, message: match change {
            Change::Time(_) => "Date & time applied and verified. Automatic time is off.",
            Change::Zone(_) => "Timezone applied and verified. Automatic timezone is off.",
            Change::Automatic { .. } => "Automatic settings verified. Android may take a moment to synchronize.",
        }.into() })
    }.await;
    if let Err(error) = result {
        // Roll back only the settings this operation touched; retain unrelated choices.
        let mut recovery = vec![];
        if value_attempted {
            let restored = match change {
                Change::Time(_) => {
                    adb.time(
                        &caps,
                        before.epoch_millis + before_at.elapsed().as_millis() as i64,
                    )
                    .await
                }
                Change::Zone(_) => adb.timezone(&caps, &before.timezone).await,
                Change::Automatic { .. } => Ok(()),
            };
            if let Err(e) = restored {
                recovery.push(e);
            }
        }
        if !matches!(change, Change::Zone(_))
            && let Err(e) = adb.automatic(&caps, false, before.automatic_time).await
        {
            recovery.push(e);
        }
        if !matches!(change, Change::Time(_))
            && let Err(e) = adb.automatic(&caps, true, before.automatic_timezone).await
        {
            recovery.push(e);
        }
        return Err(if recovery.is_empty() {
            format!("{error} Previous automatic settings restored; refresh to check device time.")
        } else {
            format!(
                "{error} Could not fully restore previous settings: {}. Reconnect and use Refresh or Open Android settings.",
                recovery.join("; ")
            )
        });
    }
    result
}
