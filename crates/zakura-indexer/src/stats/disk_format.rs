//! Ordered daily-statistics keys.

use chrono::{DateTime, NaiveDate, Utc};

use crate::Error;

const SECONDS_PER_DAY: i64 = 86_400;

pub(crate) fn day_number(timestamp: i64) -> Result<u32, Error> {
    u32::try_from(timestamp.div_euclid(SECONDS_PER_DAY)).map_err(|_| {
        Error::Calculation(format!(
            "block timestamp {timestamp} is outside the chart date range"
        ))
    })
}

pub(crate) const fn day_key(day: u32) -> [u8; 4] {
    day.to_be_bytes()
}

pub(super) fn date_to_day(date: &str) -> Result<u32, Error> {
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
        Error::InvalidQuery(format!("invalid chart date {date:?}; expected YYYY-MM-DD"))
    })?;
    let timestamp = date
        .and_hms_opt(0, 0, 0)
        .expect("midnight is always a valid time")
        .and_utc()
        .timestamp();
    day_number(timestamp)
}

pub(super) fn day_to_date(day: u32) -> Result<String, Error> {
    let timestamp = i64::from(day)
        .checked_mul(SECONDS_PER_DAY)
        .ok_or_else(|| Error::Calculation("chart date exceeds i64".to_string()))?;
    let date = DateTime::<Utc>::from_timestamp(timestamp, 0)
        .ok_or_else(|| Error::Calculation("chart date is outside chrono's range".to_string()))?;
    Ok(date.format("%Y-%m-%d").to_string())
}
