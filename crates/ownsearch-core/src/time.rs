use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn jitter(max_ms: u64) {
    if max_ms == 0 {
        return;
    }
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    let spread = u64::from_le_bytes(bytes) % max_ms.saturating_add(1);
    tokio::time::sleep(std::time::Duration::from_millis(spread)).await;
}

pub fn now_unix() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

pub fn rfc3339(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.000Z")
}

pub fn now_rfc3339() -> String {
    rfc3339(now_unix())
}

pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted_days = days.saturating_add(719_468);
    let era = shifted_days.div_euclid(146_097);
    let day_of_era = shifted_days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * month_index + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    })
    .unwrap_or(1);
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub(crate) fn parse_gmt_stamp(raw: &str) -> Option<i64> {
    let cleaned = raw.trim();
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    let month_name = parts.first()?;
    let month = u32::try_from(
        MONTHS
            .iter()
            .position(|name| name.eq_ignore_ascii_case(month_name))?,
    )
    .ok()?
    .saturating_add(1);
    let day = parts.get(1)?.parse::<u32>().ok()?;
    let clock = parts.get(2)?.split(':').collect::<Vec<_>>();
    if clock.len() != 3 {
        return None;
    }
    let hour = clock.first()?.parse::<i64>().ok()?;
    let minute = clock.get(1)?.parse::<i64>().ok()?;
    let second = clock.get(2)?.parse::<i64>().ok()?;
    let year = parts.get(3)?.parse::<i64>().ok()?;
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    days.checked_mul(86_400)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)
}

pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(crate) fn days_until(unix: i64, now: i64) -> i64 {
    (unix - now).div_euclid(86_400)
}

pub fn parse_rfc3339(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let (date, rest) = trimmed.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i64>().ok()?;
    let month = date_parts.next()?.parse::<u32>().ok()?;
    let day = date_parts.next()?.parse::<u32>().ok()?;
    if date_parts.next().is_some()
        || !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
    {
        return None;
    }
    let clock = rest.trim_end_matches('Z').split(':').collect::<Vec<_>>();
    if clock.len() != 3 {
        return None;
    }
    let hour = clock.first()?.parse::<i64>().ok()?;
    let minute = clock.get(1)?.parse::<i64>().ok()?;
    let second = clock.get(2)?.split('.').next()?.parse::<i64>().ok()?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=60).contains(&second) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    days.checked_mul(86_400)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_formats_as_a_utc_stamp() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(1_791_600_000), "2026-10-10T02:40:00.000Z");
    }

    #[test]
    fn a_stamp_round_trips() {
        let stamp = now_unix();
        let text = rfc3339(stamp);
        assert_eq!(parse_rfc3339(&text), Some(stamp));
    }

    #[test]
    fn the_searx_space_certificate_stamp_parses() {
        let unix = parse_gmt_stamp("Jan  8 17:29:56 2027 GMT").expect("stamp");
        assert_eq!(rfc3339(unix), "2027-01-08T17:29:56.000Z");
    }

    #[test]
    fn day_arithmetic_crosses_a_year_boundary() {
        let now = parse_gmt_stamp("Dec 31 00:00:00 2026 GMT").expect("now");
        let then = parse_gmt_stamp("Jan  1 00:00:00 2027 GMT").expect("then");
        assert_eq!(days_until(then, now), 1);
    }

    #[test]
    fn an_oversized_year_is_none() {
        assert_eq!(parse_gmt_stamp("Jan 1 00:00:00 999999999999999 GMT"), None);
        assert_eq!(parse_rfc3339("999999999999999-01-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2026-10-26T99:00:00Z"), None);
        assert_eq!(parse_gmt_stamp("Jan 40 00:00:00 2026 GMT"), None);
    }

    #[test]
    fn a_bad_stamp_is_none() {
        assert_eq!(parse_gmt_stamp("not a date"), None);
        assert_eq!(parse_rfc3339("2026-13"), None);
    }
}
