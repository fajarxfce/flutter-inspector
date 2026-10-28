use chrono::{DateTime, Datelike, LocalResult, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

#[derive(Clone, Debug)]
pub struct DeviceClock {
    pub epoch_millis: i64,
    pub timezone: String,
    pub automatic_time: bool,
    pub automatic_timezone: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalDateTime {
    pub date: String,
    pub time: String,
    pub offset: String,
}

pub fn timezones() -> impl Iterator<Item = &'static str> {
    chrono_tz::TZ_VARIANTS.iter().copied().map(Tz::name)
}

pub fn validate_timezone(name: &str) -> Result<(), String> {
    timezone(name).map(|_| ())
}

fn timezone(name: &str) -> Result<Tz, String> {
    name.parse()
        .map_err(|_| "Choose a valid IANA timezone, for example Asia/Jakarta.".into())
}

/// Interpret the user's wall clock in the device's zone, never the host's zone.
pub fn local_to_epoch(date: &str, time: &str, zone: &str) -> Result<i64, String> {
    let zone = timezone(zone)?;
    let value = format!("{} {}", date.trim(), time.trim());
    let local = NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M"))
        .map_err(|_| "Use a valid date (YYYY-MM-DD) and time (HH:MM or HH:MM:SS).")?;
    if !(1970..=2099).contains(&local.year()) {
        return Err("Choose a date between 1970 and 2099.".into());
    }
    match zone.from_local_datetime(&local) {
        LocalResult::Single(value) if value.timestamp_millis() >= 0 => Ok(value.timestamp_millis()),
        LocalResult::Single(_) => Err("The resulting UTC time must be on or after 1970-01-01.".into()),
        LocalResult::None => Err("This local time does not exist because of a daylight-saving transition. Choose another time.".into()),
        LocalResult::Ambiguous(_, _) => Err("This local time occurs twice during a daylight-saving transition. Choose an unambiguous time.".into()),
    }
}

pub fn epoch_to_local(epoch_millis: i64, zone: &str) -> Result<LocalDateTime, String> {
    let time = DateTime::<Utc>::from_timestamp_millis(epoch_millis)
        .ok_or("Device returned an invalid timestamp.")?
        .with_timezone(&timezone(zone)?);
    Ok(LocalDateTime {
        date: time.format("%Y-%m-%d").to_string(),
        time: time.format("%H:%M:%S").to_string(),
        offset: time.format("%:z").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_is_interpreted_in_selected_zone_and_round_trips() {
        let jakarta = local_to_epoch("2026-10-28", "14:30", "Asia/Jakarta").unwrap();
        let utc = local_to_epoch("2026-10-28", "07:30:00", "UTC").unwrap();
        assert_eq!(jakarta, utc);
        assert_eq!(
            epoch_to_local(jakarta, "Asia/Jakarta").unwrap(),
            LocalDateTime {
                date: "2026-10-28".into(),
                time: "14:30:00".into(),
                offset: "+07:00".into()
            }
        );
        assert_eq!(
            epoch_to_local(utc, "Asia/Kathmandu").unwrap().offset,
            "+05:45"
        );
    }

    #[test]
    fn rejects_invalid_dates_zones_and_dst_gaps_or_ambiguities() {
        assert!(local_to_epoch("2026-02-29", "12:00", "UTC").is_err());
        assert!(local_to_epoch("2026-10-28", "25:00", "UTC").is_err());
        assert!(local_to_epoch("1969-12-31", "12:00", "UTC").is_err());
        assert!(local_to_epoch("2026-10-28", "12:00", "Asia/Jakarta;reboot").is_err());
        assert!(
            local_to_epoch("2026-03-08", "02:30", "America/New_York")
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(
            local_to_epoch("2026-11-01", "01:30", "America/New_York")
                .unwrap_err()
                .contains("twice")
        );
    }
}
