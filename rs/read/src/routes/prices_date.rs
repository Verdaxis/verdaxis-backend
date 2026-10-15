//! Date query parsing follows Pydantic's date then datetime fallback.
use crate::ApiError;
use axum::http::StatusCode;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde_json::json;

pub fn parse(value: &str, name: &str) -> Result<NaiveDate, ApiError> {
    let error = |reason: &str| {
        failure(
            value,
            name,
            "date_from_datetime_parsing",
            &format!("Input should be a valid date or datetime, {reason}"),
            Some(reason),
        )
    };
    let inexact = || {
        failure(
            value,
            name,
            "date_from_datetime_inexact",
            "Datetimes provided to dates should have zero time - e.g. be exact dates",
            None,
        )
    };
    if !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || b"+-.".contains(&c))
        && let Ok(number) = value.parse::<f64>()
    {
        let seconds = if number.abs() > 20_000_000_000.0 {
            number / 1000.0
        } else {
            number
        };
        let instant = DateTime::<Utc>::from_timestamp(
            seconds.floor() as i64,
            ((seconds - seconds.floor()) * 1_000_000.0).round() as u32 * 1000,
        )
        .ok_or_else(|| error("dates after 9999 are not supported as unix timestamps"))?;
        if instant.year() < 1 {
            return Err(error(
                "dates before 0001 are not supported as unix timestamps",
            ));
        }
        if instant.year() > 9999 {
            return Err(error(
                "dates after 9999 are not supported as unix timestamps",
            ));
        }
        if seconds.rem_euclid(86400.0) != 0.0 {
            return Err(inexact());
        }
        return Ok(instant.date_naive());
    }
    let bytes = value.as_bytes();
    if bytes.len() < 10 {
        return Err(error("input is too short"));
    }
    let part = |start: usize, end: usize, label: &str| -> Result<u32, ApiError> {
        let slice = &bytes[start..end];
        if !slice.iter().all(u8::is_ascii_digit) {
            return Err(error(&format!("invalid character in {label}")));
        }
        Ok(slice.iter().fold(0, |n, c| n * 10 + u32::from(c - b'0')))
    };
    let year = part(0, 4, "year")?;
    if bytes[4] != b'-' {
        return Err(error("invalid date separator, expected `-`"));
    }
    let month = part(5, 7, "month")?;
    if bytes[7] != b'-' {
        return Err(error("invalid date separator, expected `-`"));
    }
    let day = part(8, 10, "day")?;
    if !(1..=12).contains(&month) {
        return Err(error("month value is outside expected range of 1-12"));
    }
    let date = NaiveDate::from_ymd_opt(year as i32, month, day)
        .ok_or_else(|| error("day value is outside expected range"))?;
    if year == 0 {
        return Err(failure(
            value,
            name,
            "date_parsing",
            "Input should be a valid date in the format YYYY-MM-DD, year 0 is out of range",
            Some("year 0 is out of range"),
        ));
    }
    if bytes.len() == 10 {
        return Ok(date);
    }
    if !b"Tt_ ".contains(&bytes[10]) {
        return Err(error(
            "invalid datetime separator, expected `T`, `t`, `_` or space",
        ));
    }
    if bytes.len() < 16 {
        return Err(error("input is too short"));
    }
    let hour = part(11, 13, "hour")?;
    if bytes[13] != b':' {
        return Err(error("invalid time separator, expected `:`"));
    }
    let minute = part(14, 16, "minute")?;
    if hour > 23 {
        return Err(error("hour value is outside expected range of 0-23"));
    }
    if minute > 59 {
        return Err(error("minute value is outside expected range of 0-59"));
    }
    let mut index = 16;
    let mut second = 0;
    let mut fraction = false;
    if bytes.get(index) == Some(&b':') {
        if bytes.len() < 19 {
            return Err(error("input is too short"));
        }
        second = part(17, 19, "second")?;
        index = 19;
        if second > 59 {
            return Err(error("second value is outside expected range of 0-59"));
        }
        if matches!(bytes.get(index), Some(b'.' | b',')) {
            index += 1;
            let start = index;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                if index - start < 6 && bytes[index] != b'0' {
                    fraction = true;
                }
                index += 1;
            }
            if index == start {
                return Err(error("second fraction digits missing after `.`"));
            }
        }
    }
    if index < bytes.len() {
        match bytes[index] {
            b'Z' | b'z' => {
                index += 1;
            }
            b'+' | b'-' => {
                if bytes.len() < index + 3 {
                    return Err(error("invalid timezone hour"));
                }
                let zone_hour = part(index + 1, index + 3, "timezone hour")?;
                index += 3;
                if bytes.get(index) == Some(&b':') {
                    index += 1;
                }
                if bytes.len() < index + 2 {
                    return Err(error("invalid timezone minute"));
                }
                let zone_minute = part(index, index + 2, "timezone minute")?;
                index += 2;
                if zone_minute > 59 {
                    return Err(error(
                        "timezone minute value is outside expected range of 0-59",
                    ));
                }
                if zone_hour > 23 {
                    return Err(error("timezone offset must be less than 24 hours"));
                }
            }
            _ => return Err(error("invalid timezone sign")),
        }
        if index != bytes.len() {
            return Err(error("unexpected extra characters at the end of the input"));
        }
    }
    if hour != 0 || minute != 0 || second != 0 || fraction {
        return Err(inexact());
    }
    Ok(date)
}
fn failure(value: &str, name: &str, kind: &str, message: &str, reason: Option<&str>) -> ApiError {
    let mut entry = json!({"type":kind,"loc":["query",name],"msg":message,"input":value});
    if let Some(reason) = reason {
        entry["ctx"] = json!({"error":reason});
    }
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        detail: json!([entry]),
        retry_after: None,
    }
}
