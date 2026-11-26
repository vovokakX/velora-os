//! Calendar arithmetic and CMOS RTC value decoding. Times are UTC.

#![cfg_attr(not(test), no_std)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    pub fn is_valid(&self) -> bool {
        (1..=12).contains(&self.month)
            && self.day >= 1
            && self.day <= days_in_month(self.year, self.month)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60
    }

    /// Seconds since 1970-01-01 00:00:00 UTC.
    pub fn to_unix(&self) -> i64 {
        let days = days_from_civil(self.year, self.month as u32, self.day as u32);
        days * 86_400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64
    }

    pub fn from_unix(t: i64) -> Self {
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month: month as u8,
            day: day as u8,
            hour: (secs / 3600) as u8,
            minute: (secs % 3600 / 60) as u8,
            second: (secs % 60) as u8,
        }
    }

    /// 0 = Sunday.
    pub fn weekday(&self) -> u8 {
        let days = days_from_civil(self.year, self.month as u32, self.day as u32);
        (days + 4).rem_euclid(7) as u8
    }
}

impl core::fmt::Display for DateTime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

pub fn bcd(v: u8) -> u8 {
    (v >> 4) * 10 + (v & 0x0f)
}

/// Raw CMOS RTC registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtcRegisters {
    pub second: u8,
    pub minute: u8,
    pub hour: u8,
    pub day: u8,
    pub month: u8,
    pub year: u8,
    /// 0 if the platform has no century register.
    pub century: u8,
    /// Status register B: bit 1 = 24-hour mode, bit 2 = binary (not BCD).
    pub status_b: u8,
}

impl RtcRegisters {
    pub fn decode(&self) -> DateTime {
        let binary = self.status_b & 0x04 != 0;
        let h24 = self.status_b & 0x02 != 0;
        let conv = |v: u8| if binary { v } else { bcd(v) };
        let pm = self.hour & 0x80 != 0;
        let mut hour = conv(self.hour & 0x7f);
        if !h24 {
            hour %= 12;
            if pm {
                hour += 12;
            }
        }
        let year2 = conv(self.year) as i32;
        let year = if self.century != 0 {
            conv(self.century) as i32 * 100 + year2
        } else if year2 < 70 {
            2000 + year2
        } else {
            1900 + year2
        };
        DateTime {
            year,
            month: conv(self.month),
            day: conv(self.day),
            hour,
            minute: conv(self.minute),
            second: conv(self.second),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_roundtrip_and_known_values() {
        let epoch = DateTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        };
        assert_eq!(epoch.to_unix(), 0);
        let t = DateTime {
            year: 2026,
            month: 11,
            day: 26,
            hour: 12,
            minute: 34,
            second: 56,
        };
        assert_eq!(t.to_unix(), 1_795_696_496);
        assert_eq!(DateTime::from_unix(1_795_696_496), t);
        let leap = DateTime {
            year: 2024,
            month: 2,
            day: 29,
            hour: 23,
            minute: 59,
            second: 59,
        };
        assert_eq!(DateTime::from_unix(leap.to_unix()), leap);
        assert_eq!(DateTime::from_unix(-1).year, 1969);
        for t in (0..4_000_000_000i64).step_by(86_399_123) {
            assert_eq!(DateTime::from_unix(t).to_unix(), t);
        }
    }

    #[test]
    fn weekdays_and_validity() {
        assert_eq!(DateTime::from_unix(0).weekday(), 4); // Thursday
        let t = DateTime {
            year: 2026,
            month: 11,
            day: 26,
            hour: 0,
            minute: 0,
            second: 0,
        };
        assert_eq!(t.weekday(), 4);
        assert!(t.is_valid());
        assert!(!DateTime { day: 31, ..t }.is_valid());
        assert!(
            !DateTime {
                year: 2025,
                month: 2,
                day: 29,
                ..t
            }
            .is_valid()
        );
        assert_eq!(format!("{}", t), "2026-11-26 00:00:00 UTC");
    }

    #[test]
    fn decode_bcd_and_binary() {
        let r = RtcRegisters {
            second: 0x56,
            minute: 0x34,
            hour: 0x12,
            day: 0x26,
            month: 0x11,
            year: 0x26,
            century: 0x20,
            status_b: 0x02,
        };
        assert_eq!(
            r.decode(),
            DateTime {
                year: 2026,
                month: 11,
                day: 26,
                hour: 12,
                minute: 34,
                second: 56
            }
        );
        let b = RtcRegisters {
            second: 56,
            minute: 34,
            hour: 12,
            day: 26,
            month: 11,
            year: 26,
            century: 0,
            status_b: 0x06,
        };
        assert_eq!(b.decode().year, 2026);
        // 12-hour mode: 0x81 = 1 PM, 0x12 = 12 AM.
        let pm = RtcRegisters {
            hour: 0x81,
            status_b: 0,
            ..r
        };
        assert_eq!(pm.decode().hour, 13);
        let midnight = RtcRegisters {
            hour: 0x12,
            status_b: 0,
            ..r
        };
        assert_eq!(midnight.decode().hour, 0);
        let noon = RtcRegisters {
            hour: 0x92,
            status_b: 0,
            ..r
        };
        assert_eq!(noon.decode().hour, 12);
    }
}
