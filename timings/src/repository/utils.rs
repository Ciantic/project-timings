use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};

use crate::Error;


pub fn datetime_to_ms(dt: &DateTime<Utc>) -> i64 {
    dt.timestamp() * 1000 + (dt.timestamp_subsec_millis() as i64)
}

pub fn ms_to_datetime(ms: i64) -> Result<DateTime<Utc>, Error> {
    let secs = ms / 1000;
    let millis = (ms % 1000) as u32;
    DateTime::<Utc>::from_timestamp(secs, millis * 1_000_000).ok_or_else(|| {
        Error::ChronoError(format!(
            "Failed to create DateTime from timestamp: secs={}, millis={}",
            secs, millis
        ))
    })
}

pub fn parse_local_date(date_str: &str) -> Result<DateTime<Local>, Error> {
    let naivedate = NaiveDate::parse_from_str(date_str, "%Y-%m-%d").map_err(|e| {
        Error::ChronoError(format!(
            "Failed to parse date string: {}: {}",
            date_str, e
        ))
    })?;

    let offset = Local.offset_from_local_date(&naivedate)
        .earliest()
        .ok_or_else(|| {
            Error::ChronoError(format!(
                "Failed to get offset for local date: {}",
                date_str
            ))
        })?;

    let midnight = naivedate.and_hms_opt(0, 0, 0).ok_or(
            Error::ChronoError("nope".to_string())
        )?;

    Ok(DateTime::<Local>::from_naive_utc_and_offset(
        midnight,
        offset,
    ))
}
