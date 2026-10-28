use inspector_adapters::device_clock::execute;
use inspector_application::ClockAction;
use inspector_domain::epoch_to_local;
use std::{path::Path, time::Instant};

#[tokio::test]
#[ignore = "Changes an isolated Android emulator's clock; set INSPECTOR_CLOCK_TEST_DEVICE explicitly"]
async fn real_android_clock_changes_and_restoration() {
    let serial =
        std::env::var("INSPECTOR_CLOCK_TEST_DEVICE").expect("Set INSPECTOR_CLOCK_TEST_DEVICE");
    assert!(
        serial.starts_with("emulator-"),
        "Only use an isolated test emulator"
    );
    let adb = Path::new("adb");
    let baseline = execute(adb, &serial, ClockAction::Read)
        .await
        .unwrap()
        .clock;
    let started = Instant::now();
    let result: Result<(), String> = async {
        let zone = execute(
            adb,
            &serial,
            ClockAction::SetTimezone("Asia/Kathmandu".into()),
        )
        .await?
        .clock;
        if zone.timezone != "Asia/Kathmandu" || zone.automatic_timezone {
            return Err("Timezone was not applied".into());
        }
        let manual = execute(
            adb,
            &serial,
            ClockAction::SetDateTime {
                date: "2028-02-29".into(),
                time: "13:14:15".into(),
                timezone: zone.timezone,
            },
        )
        .await?
        .clock;
        let local = epoch_to_local(manual.epoch_millis, &manual.timezone)?;
        if local.date != "2028-02-29" || !local.time.starts_with("13:14:") || manual.automatic_time
        {
            return Err(format!("Wrong manual time: {local:?}"));
        }
        let synced = execute(adb, &serial, ClockAction::SyncComputer)
            .await?
            .clock;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        if synced.epoch_millis.abs_diff(now) > 5000 {
            return Err("Computer sync was not applied".into());
        }
        let auto = execute(
            adb,
            &serial,
            ClockAction::SetAutomatic {
                time: true,
                timezone: true,
            },
        )
        .await?
        .clock;
        if !auto.automatic_time || !auto.automatic_timezone {
            return Err("Automatic settings were not restored".into());
        }
        Ok(())
    }
    .await;
    // Restore even after an operation fails; all changes target only this emulator.
    let zone_restore = execute(
        adb,
        &serial,
        ClockAction::SetTimezone(baseline.timezone.clone()),
    )
    .await;
    let local = epoch_to_local(
        baseline.epoch_millis + started.elapsed().as_millis() as i64,
        &baseline.timezone,
    )
    .unwrap();
    let time_restore = execute(
        adb,
        &serial,
        ClockAction::SetDateTime {
            date: local.date,
            time: local.time,
            timezone: baseline.timezone,
        },
    )
    .await;
    let auto_restore = execute(
        adb,
        &serial,
        ClockAction::SetAutomatic {
            time: baseline.automatic_time,
            timezone: baseline.automatic_timezone,
        },
    )
    .await;
    assert!(result.is_ok(), "Clock operations failed: {result:?}");
    assert!(
        zone_restore.is_ok() && time_restore.is_ok() && auto_restore.is_ok(),
        "Restoration failed: {zone_restore:?}, {time_restore:?}, {auto_restore:?}"
    );
}
