pub(crate) fn format_relative_time(value: &str) -> String {
    if value.is_empty() {
        return value.to_string();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(ts) = parse_iso_timestamp(value) {
        let diff = now.saturating_sub(ts);
        let minutes = diff / 60;
        let hours = diff / 3600;
        let days = diff / 86400;

        if minutes < 1 {
            return "just now".to_string();
        }
        if minutes < 60 {
            return format!("{minutes}m ago");
        }
        if hours < 24 {
            return format!("{hours}h ago");
        }
        if days < 30 {
            return format!("{days}d ago");
        }
    }

    if value.len() > 10 {
        value[..10].to_string()
    } else {
        value.to_string()
    }
}

fn parse_iso_timestamp(value: &str) -> Option<u64> {
    let parts: Vec<&str> = value.split('T').collect();
    if parts.len() < 2 {
        return None;
    }
    let date_parts: Vec<u64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    if date_parts.len() != 3 {
        return None;
    }
    let time_str = parts[1].trim_end_matches('Z');
    let time_parts: Vec<u64> = time_str.split(':').filter_map(|p| p.parse().ok()).collect();
    if time_parts.len() < 2 {
        return None;
    }

    let year = date_parts[0];
    let month = date_parts[1];
    let day = date_parts[2];
    let hour = time_parts[0];
    let minute = time_parts[1];
    let second = if time_parts.len() > 2 {
        time_parts[2]
    } else {
        0
    };

    let mut days_total: u64 = 0;
    for y in 1970..year {
        days_total += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    for m in 1..month {
        days_total += month_days[m as usize];
        if m == 2 && is_leap {
            days_total += 1;
        }
    }
    days_total += day - 1;

    Some(days_total * 86400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_iso_timestamp_handles_epoch_and_leap_days() {
        assert_eq!(parse_iso_timestamp("1970-01-01T00:01:02Z"), Some(62));
        assert_eq!(
            parse_iso_timestamp("1972-03-01T00:00:00Z"),
            Some((365 + 365 + 31 + 29) * 86400)
        );
    }

    #[test]
    fn format_relative_time_preserves_empty_and_unknown_values() {
        assert_eq!(format_relative_time(""), "");
        assert_eq!(format_relative_time("unknown"), "unknown");
    }
}
