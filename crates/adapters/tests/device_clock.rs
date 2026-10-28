#![cfg(unix)]
use inspector_adapters::device_clock::execute;
use inspector_application::ClockAction;
use inspector_domain::local_to_epoch;
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture {
    root: tempfile::TempDir,
    adb: PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let adb = root.path().join("adb");
        std::fs::write(&adb, include_str!("../../../tools/fixtures/clock_adb.py")).unwrap();
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            root.path().join("state.json"),
            json!({
                "epoch_millis": 1793150000000_i64, "timezone":"Asia/Jakarta",
                "auto_time":true, "auto_time_zone":true, "mode":mode
            })
            .to_string(),
        )
        .unwrap();
        Self { root, adb }
    }
    fn state(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.root.path().join("state.json")).unwrap())
            .unwrap()
    }
    fn edit(&self, key: &str, value: Value) {
        let mut state = self.state();
        state[key] = value;
        std::fs::write(self.root.path().join("state.json"), state.to_string()).unwrap();
    }
    fn calls(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.path().join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    async fn run(&self, action: ClockAction) -> Result<inspector_application::ClockResult, String> {
        execute(&self.adb, "emulator-clock", action).await
    }
}

#[tokio::test]
async fn reads_are_passive_and_both_android_command_paths_apply_and_verify() {
    for mode in ["modern", "legacy", "detector_only"] {
        let fixture = Fixture::new(mode);
        let before = fixture.run(ClockAction::Read).await.unwrap().clock;
        assert_eq!(fixture.calls().len(), 4);
        assert_eq!(before.timezone, "Asia/Jakarta");
        let zone = fixture
            .run(ClockAction::SetTimezone("Asia/Kathmandu".into()))
            .await
            .unwrap()
            .clock;
        assert!(!zone.automatic_timezone);
        assert!(zone.automatic_time);
        assert_eq!(zone.epoch_millis, before.epoch_millis);
        let changed = fixture
            .run(ClockAction::SetDateTime {
                date: "2027-06-14".into(),
                time: "17:42:30".into(),
                timezone: zone.timezone.clone(),
            })
            .await
            .unwrap()
            .clock;
        assert!(!changed.automatic_time);
        assert!(
            changed
                .epoch_millis
                .abs_diff(local_to_epoch("2027-06-14", "17:42:30", "Asia/Kathmandu").unwrap())
                < 2000
        );
        let synced = fixture.run(ClockAction::SyncComputer).await.unwrap().clock;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(synced.epoch_millis.abs_diff(now) < 2000);
        let restored = fixture
            .run(ClockAction::SetAutomatic {
                time: true,
                timezone: true,
            })
            .await
            .unwrap()
            .clock;
        assert!(restored.automatic_time && restored.automatic_timezone);
        fixture.run(ClockAction::OpenSettings).await.unwrap();
    }
}

#[tokio::test]
async fn rejects_bad_input_before_any_writes_and_detects_external_timezone_change() {
    let fixture = Fixture::new("modern");
    assert!(
        fixture
            .run(ClockAction::SetTimezone("UTC; reboot".into()))
            .await
            .is_err()
    );
    assert!(fixture.calls().is_empty());
    assert!(
        fixture
            .run(ClockAction::SetDateTime {
                date: "2026-02-30".into(),
                time: "12:00".into(),
                timezone: "UTC".into()
            })
            .await
            .is_err()
    );
    assert!(fixture.calls().is_empty());
    let error = fixture
        .run(ClockAction::SetDateTime {
            date: "2026-11-01".into(),
            time: "12:00".into(),
            timezone: "UTC".into(),
        })
        .await
        .unwrap_err();
    assert!(error.contains("timezone changed"));
    assert!(fixture.state()["auto_time"].as_bool().unwrap());
    assert!(
        !fixture
            .calls()
            .iter()
            .any(|v| v.to_string().contains("set_auto_detection_enabled"))
    );
    assert!(
        execute(&fixture.adb, "-e", ClockAction::SyncComputer)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn permission_errors_and_silent_rejections_restore_automatic_settings() {
    for (key, value) in [
        ("reject_next", "time"),
        ("noop", "time"),
        ("reject_next", "timezone"),
        ("noop", "timezone"),
    ] {
        let fixture = Fixture::new("modern");
        fixture.edit(key, json!(value));
        let action = if value == "time" {
            ClockAction::SetDateTime {
                date: "2029-01-01".into(),
                time: "12:00".into(),
                timezone: "Asia/Jakarta".into(),
            }
        } else {
            ClockAction::SetTimezone("Europe/London".into())
        };
        assert!(fixture.run(action).await.is_err());
        let after = fixture.run(ClockAction::Read).await.unwrap().clock;
        assert!(after.automatic_time && after.automatic_timezone);
        assert_eq!(after.timezone, "Asia/Jakarta");
        assert!(after.epoch_millis.abs_diff(1793150000000_i64) < 3000);
    }
    let fixture = Fixture::new("unsupported");
    assert!(
        fixture
            .run(ClockAction::SetTimezone("UTC".into()))
            .await
            .unwrap_err()
            .contains("unavailable")
    );
    assert!(fixture.state()["auto_time_zone"].as_bool().unwrap());
}

#[tokio::test]
async fn timed_out_adb_is_killed_and_a_later_read_can_recover() {
    let fixture = Fixture::new("modern");
    std::fs::write(fixture.root.path().join("hang"), "").unwrap();
    let start = std::time::Instant::now();
    assert!(
        fixture
            .run(ClockAction::Read)
            .await
            .unwrap_err()
            .contains("timed out")
    );
    assert!(start.elapsed().as_secs() < 7);
    std::fs::remove_file(fixture.root.path().join("hang")).unwrap();
    fixture.run(ClockAction::Read).await.unwrap();
    #[cfg(target_os = "linux")]
    {
        let pid = std::fs::read_to_string(fixture.root.path().join("hung.pid")).unwrap();
        let status = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(status.is_empty() || status.split(')').nth(1).unwrap().starts_with(" Z"));
    }
}
