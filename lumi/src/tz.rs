//! Local time from the region chosen in the installer: a standard offset
//! and the daylight saving rule of the region's main time zone. The
//! kernel's clock is UTC (from the RTC).

use alloc::format;
use alloc::string::String;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dst {
    None,
    /// Last Sunday of March to the last Sunday of October, 01:00 UTC.
    Eu,
    /// Second Sunday of March to the first Sunday of November, 2:00 local.
    Us,
}

/// (region, standard offset in minutes, rule, zone name).
const ZONES: [(&str, i32, Dst, &str); 12] = [
    ("UA", 120, Dst::Eu, "Europe/Kyiv"),
    ("RU", 180, Dst::None, "Europe/Moscow"),
    ("BY", 180, Dst::None, "Europe/Minsk"),
    ("KZ", 300, Dst::None, "Asia/Almaty"),
    ("PL", 60, Dst::Eu, "Europe/Warsaw"),
    ("DE", 60, Dst::Eu, "Europe/Berlin"),
    ("US", -300, Dst::Us, "America/New_York"),
    ("GB", 0, Dst::Eu, "Europe/London"),
    ("FR", 60, Dst::Eu, "Europe/Paris"),
    ("ES", 60, Dst::Eu, "Europe/Madrid"),
    ("IT", 60, Dst::Eu, "Europe/Rome"),
    ("TR", 180, Dst::None, "Europe/Istanbul"),
];

/// Regions in the installer's order with Russian and English names.
pub const REGIONS: [(&str, &str, &str); 12] = [
    ("UA", "Украина", "Ukraine"),
    ("RU", "Россия", "Russia"),
    ("BY", "Беларусь", "Belarus"),
    ("KZ", "Казахстан", "Kazakhstan"),
    ("PL", "Польша", "Poland"),
    ("DE", "Германия", "Germany"),
    ("US", "США", "United States"),
    ("GB", "Великобритания", "United Kingdom"),
    ("FR", "Франция", "France"),
    ("ES", "Испания", "Spain"),
    ("IT", "Италия", "Italy"),
    ("TR", "Турция", "Türkiye"),
];

/// The zone name of a region (UTC if it is unknown).
pub fn zone_name(region: &str) -> &'static str {
    ZONES.iter().find(|z| z.0 == region).map_or("UTC", |z| z.3)
}

/// Days since 1970-01-01 to (year, month 1..=12, day 1..=31).
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since 1970-01-01 of a date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 0 = Monday ... 6 = Sunday.
pub fn weekday(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

/// The day of the `n`-th (1-based) Sunday of a month, or the last one
/// with `n` = 0.
fn sunday(y: i64, m: u32, n: u32) -> i64 {
    if n == 0 {
        let next = if m == 12 {
            days_from_civil(y + 1, 1, 1)
        } else {
            days_from_civil(y, m + 1, 1)
        };
        let last = next - 1;
        return last - ((weekday(last) + 1) % 7) as i64;
    }
    let first = days_from_civil(y, m, 1);
    let to_sun = (6 - weekday(first)) as i64;
    first + to_sun + 7 * (n as i64 - 1)
}

/// The offset from UTC in minutes at UTC time `t` (seconds) in a region.
pub fn offset_minutes(region: &str, t: i64) -> i32 {
    let Some(&(_, std, dst, _)) = ZONES.iter().find(|z| z.0 == region) else {
        return 0;
    };
    let (y, _, _) = civil(t.div_euclid(86_400));
    let summer = match dst {
        Dst::None => false,
        Dst::Eu => {
            let start = sunday(y, 3, 0) * 86_400 + 3600;
            let end = sunday(y, 10, 0) * 86_400 + 3600;
            t >= start && t < end
        }
        Dst::Us => {
            // 2:00 local standard time = 7:00 UTC for New York.
            let at = |d: i64| d * 86_400 + 7200 - std as i64 * 60;
            t >= at(sunday(y, 3, 2)) && t < at(sunday(y, 11, 1)) - 3600
        }
    };
    std + if summer { 60 } else { 0 }
}

/// Local time of a region at UTC `t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Local {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub weekday: u32,
}

pub fn local(region: &str, t: i64) -> Local {
    let l = t + offset_minutes(region, t) as i64 * 60;
    let days = l.div_euclid(86_400);
    let s = l.rem_euclid(86_400);
    let (year, month, day) = civil(days);
    Local {
        year,
        month,
        day,
        hour: (s / 3600) as u32,
        minute: (s / 60 % 60) as u32,
        second: (s % 60) as u32,
        weekday: weekday(days),
    }
}

impl Local {
    pub fn hhmm(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// "28 ноя" / "28 Nov".
    pub fn short_date(&self, ru: bool) -> String {
        const RU: [&str; 12] = [
            "янв", "фев", "мар", "апр", "мая", "июн", "июл", "авг", "сен", "окт", "ноя", "дек",
        ];
        const EN: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let m = (self.month as usize).clamp(1, 12) - 1;
        format!("{} {}", self.day, if ru { RU[m] } else { EN[m] })
    }

    /// "суббота, 28 ноября" / "Saturday, 28 November".
    pub fn date(&self, ru: bool) -> String {
        const WD_RU: [&str; 7] = [
            "понедельник",
            "вторник",
            "среда",
            "четверг",
            "пятница",
            "суббота",
            "воскресенье",
        ];
        const WD_EN: [&str; 7] = [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ];
        const M_RU: [&str; 12] = [
            "января",
            "февраля",
            "марта",
            "апреля",
            "мая",
            "июня",
            "июля",
            "августа",
            "сентября",
            "октября",
            "ноября",
            "декабря",
        ];
        const M_EN: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        let w = self.weekday.min(6) as usize;
        let m = (self.month.clamp(1, 12) - 1) as usize;
        if ru {
            format!("{}, {} {}", WD_RU[w], self.day, M_RU[m])
        } else {
            format!("{}, {} {}", WD_EN[w], self.day, M_EN[m])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_and_zones() {
        assert_eq!(civil(0), (1970, 1, 1));
        let d = days_from_civil(2026, 11, 28);
        assert_eq!(civil(d), (2026, 11, 28));
        assert_eq!(weekday(d), 5, "28 November 2026 is a Saturday");
        // Kyiv: winter +2, summer +3 (from 29 March 2026, 01:00 UTC).
        let winter = d * 86_400 + 12 * 3600;
        assert_eq!(offset_minutes("UA", winter), 120);
        let switch = days_from_civil(2026, 3, 29) * 86_400 + 3600;
        assert_eq!(offset_minutes("UA", switch - 1), 120);
        assert_eq!(offset_minutes("UA", switch), 180);
        // New York: summer from 8 March 2026, 07:00 UTC.
        let ny = days_from_civil(2026, 3, 8) * 86_400 + 7 * 3600;
        assert_eq!(offset_minutes("US", ny - 1), -300);
        assert_eq!(offset_minutes("US", ny), -240);
        let l = local("UA", winter);
        assert_eq!((l.hour, l.minute), (14, 0));
        assert_eq!(l.date(true), "суббота, 28 ноября");
        assert_eq!(local("??", winter).hour, 12, "unknown regions use UTC");
    }
}
