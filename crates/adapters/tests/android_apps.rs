#![cfg(unix)]
use inspector_adapters::android_apps::{execute, list};
use inspector_application::{AppAction, AppSettingsPage};
use inspector_domain::AppTarget;
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    root: tempfile::TempDir,
    adb: PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let adb = root.path().join("adb");
        // Share an immutable executable; parallel fork/exec must not inherit a
        // temporary writer for a just-created script (Linux ETXTBSY).
        std::os::unix::fs::symlink(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/fixtures/apps_adb.py"),
            &adb,
        )
        .unwrap();
        std::fs::write(
            root.path().join("apps-state.json"),
            json!({"mode":mode}).to_string(),
        )
        .unwrap();
        Self { root, adb }
    }
    fn state(&self) -> Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.root.path().join("apps-state.json")).unwrap(),
        )
        .unwrap()
    }
    fn calls(&self) -> String {
        std::fs::read_to_string(self.root.path().join("apps-calls.jsonl")).unwrap_or_default()
    }
    fn target(&self) -> AppTarget {
        AppTarget {
            serial: "emulator-apps".into(),
            user: 0,
            package: "dev.example.alpha".into(),
        }
    }
    async fn run(&self, action: AppAction) -> Result<inspector_application::AppResult, String> {
        execute(&self.adb, &self.target(), action).await
    }
}

#[tokio::test]
async fn lists_user_and_system_apps_without_mutation_and_keeps_user_permissions_separate() {
    let f = Fixture::new("different_users");
    let listed = list(&f.adb, "emulator-apps").await.unwrap();
    assert_eq!(listed.user, 0);
    assert!(listed.cache_only);
    assert_eq!(listed.apps.iter().filter(|a| a.system).count(), 2);
    assert_eq!(listed.apps.iter().filter(|a| !a.system).count(), 2);
    let details = f.run(AppAction::Inspect).await.unwrap().details.unwrap();
    let camera = details
        .permissions
        .iter()
        .find(|p| p.name.ends_with("CAMERA"))
        .unwrap();
    assert_eq!(camera.granted, Some(false));
    assert!(camera.editable());
    assert!(
        details
            .permissions
            .iter()
            .filter(|p| !p.name.ends_with("CAMERA"))
            .all(|p| !p.editable())
    );
    let mut other = f.target();
    other.user = 10;
    let details = execute(&f.adb, &other, AppAction::Inspect)
        .await
        .unwrap()
        .details
        .unwrap();
    assert_eq!(
        details
            .permissions
            .iter()
            .find(|p| p.name.ends_with("CAMERA"))
            .unwrap()
            .granted,
        Some(true)
    );
    assert_eq!(f.state(), json!({"mode":"different_users"}));
}

#[tokio::test]
async fn permissions_apply_and_read_back_rejections_and_silent_noops_are_errors() {
    for granted in [true, false] {
        let f = Fixture::new("normal");
        let result = f
            .run(AppAction::SetPermission {
                name: "android.permission.CAMERA".into(),
                granted,
            })
            .await
            .unwrap();
        assert_eq!(
            result
                .details
                .unwrap()
                .permissions
                .iter()
                .find(|p| p.name.ends_with("CAMERA"))
                .unwrap()
                .granted,
            Some(granted)
        );
    }
    for mode in ["noop", "reject"] {
        let f = Fixture::new(mode);
        assert!(
            f.run(AppAction::SetPermission {
                name: "android.permission.CAMERA".into(),
                granted: true
            })
            .await
            .is_err()
        );
    }
    let f = Fixture::new("normal");
    for name in [
        "android.permission.INTERNET",
        "android.permission.POST_NOTIFICATIONS",
        "android.permission.UNKNOWN",
    ] {
        assert!(
            f.run(AppAction::SetPermission {
                name: name.into(),
                granted: true
            })
            .await
            .is_err()
        );
    }
    assert!(!f.calls().contains("\"grant\""));
}

#[tokio::test]
async fn cache_only_preserves_data_storage_resets_only_the_target_and_legacy_never_falls_back_to_data_clear()
 {
    let f = Fixture::new("normal");
    f.run(AppAction::ClearCache).await.unwrap();
    let state = f.state();
    let app = &state["apps"]["emulator-apps:0:dev.example.alpha"];
    assert_eq!(app["cache"], 0);
    assert_eq!(app["data"], 99);
    f.run(AppAction::ClearStorage).await.unwrap();
    let state = f.state();
    let app = &state["apps"]["emulator-apps:0:dev.example.alpha"];
    assert_eq!(app["data"], 0);
    assert_eq!(state["apps"].as_object().unwrap().len(), 1);
    let f = Fixture::new("legacy");
    assert!(!list(&f.adb, "emulator-apps").await.unwrap().cache_only);
    assert!(f.run(AppAction::ClearCache).await.is_err());
    assert!(!f.calls().contains("\"clear\""));
    let f = Fixture::new("reject");
    assert!(f.run(AppAction::ClearStorage).await.is_err());
}

#[tokio::test]
async fn invalid_targets_never_reach_shell_and_post_write_read_failure_is_explicit() {
    let f = Fixture::new("normal");
    let mut target = f.target();
    target.package = "dev.example;reboot".into();
    assert!(
        execute(&f.adb, &target, AppAction::ClearStorage)
            .await
            .is_err()
    );
    assert!(
        f.run(AppAction::SetPermission {
            name: "android.permission.CAMERA;reboot".into(),
            granted: true
        })
        .await
        .is_err()
    );
    assert!(f.calls().is_empty());
    let f = Fixture::new("readback");
    let result = f.run(AppAction::ClearCache).await.unwrap();
    assert!(result.warning);
    assert!(result.details.is_none());
    assert!(result.message.contains("reading the new state failed"));
}

#[tokio::test]
async fn force_stop_and_launch_are_scoped_to_selected_user() {
    let f = Fixture::new("normal");
    let mut target = f.target();
    target.user = 10;
    assert!(
        execute(&f.adb, &target, AppAction::ForceStop)
            .await
            .unwrap()
            .details
            .unwrap()
            .stopped
    );
    assert!(
        !execute(&f.adb, &target, AppAction::Launch)
            .await
            .unwrap()
            .details
            .unwrap()
            .stopped
    );
    assert_eq!(
        f.state()["apps"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["emulator-apps:10:dev.example.alpha"]
    );
}

#[tokio::test]
async fn settings_intents_address_the_selected_package_without_changing_permissions() {
    for page in [
        AppSettingsPage::Permissions,
        AppSettingsPage::Notifications,
        AppSettingsPage::Overlay,
    ] {
        let f = Fixture::new("normal");
        f.run(AppAction::OpenSettings(page)).await.unwrap();
        assert_eq!(f.state(), json!({"mode":"normal"}));
        let calls: Vec<Value> = f
            .calls()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let intent = calls.last().unwrap().as_array().unwrap();
        assert!(
            intent
                .windows(2)
                .any(|w| w == [json!("--user"), json!("0")])
        );
        assert!(intent.windows(2).any(|w| w
            == [
                json!("android.provider.extra.APP_PACKAGE"),
                json!("dev.example.alpha")
            ]));
        // Permission/notification activity filters expect extras, no data URI.
        assert_eq!(
            intent.contains(&json!("-d")),
            matches!(page, AppSettingsPage::Overlay)
        );
    }
}

#[tokio::test]
async fn output_and_time_are_bounded_and_timeout_recovers() {
    let f = Fixture::new("large");
    assert!(list(&f.adb, "emulator-apps").await.is_err());
    let f = Fixture::new("hang");
    let started = std::time::Instant::now();
    assert!(
        list(&f.adb, "emulator-apps")
            .await
            .unwrap_err()
            .contains("timed out")
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(7));
    std::fs::write(
        f.root.path().join("apps-state.json"),
        json!({"mode":"normal"}).to_string(),
    )
    .unwrap();
    assert!(list(&f.adb, "emulator-apps").await.is_ok());
}

#[tokio::test]
#[ignore = "Read-only device probe; requires INSPECTOR_APPS_TEST_DEVICE and INSPECTOR_APPS_TEST_PACKAGE"]
async fn connected_android_read_only() {
    let serial = std::env::var("INSPECTOR_APPS_TEST_DEVICE").expect("explicit device required");
    let package = std::env::var("INSPECTOR_APPS_TEST_PACKAGE").expect("explicit package required");
    let adb = std::env::var_os("INSPECTOR_ADB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("adb"));
    let apps = list(&adb, &serial).await.unwrap();
    assert!(apps.apps.iter().any(|a| a.package == package));
    let result = execute(
        &adb,
        &AppTarget {
            serial,
            package,
            user: apps.user,
        },
        AppAction::Inspect,
    )
    .await
    .unwrap();
    assert!(!result.details.unwrap().permissions.is_empty());
    println!(
        "Read-only package list and selected-app permission parsing passed ({} packages)",
        apps.apps.len()
    );
}
