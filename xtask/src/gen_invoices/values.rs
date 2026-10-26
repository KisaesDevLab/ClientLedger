//! Dates and amounts as printed on synthetic documents. Amounts are whole
//! cents (`i64`); nothing here uses floating point for money.

/// A calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub y: i32,
    pub m: u32,
    pub d: u32,
}

const MONTHS: [&str; 12] = [
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

impl Date {
    /// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
    pub fn days(self) -> i64 {
        let y = i64::from(self.y) - i64::from(self.m <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = i64::from(self.m);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(self.d) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub fn from_days(z: i64) -> Date {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        Date {
            y: i32::try_from(y).unwrap_or(2026),
            m: u32::try_from(m).unwrap_or(1),
            d: u32::try_from(d).unwrap_or(1),
        }
    }

    pub fn plus(self, days: i64) -> Date {
        Date::from_days(self.days() + days)
    }

    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }

    fn month_name(self) -> &'static str {
        MONTHS[usize::try_from(self.m.clamp(1, 12) - 1).unwrap_or(0)]
    }

    pub fn format(self, f: DateFmt) -> String {
        let (y, m, d) = (self.y, self.m, self.d);
        let mon = self.month_name();
        match f {
            DateFmt::Mdy => format!("{m:02}/{d:02}/{y}"),
            DateFmt::MdyShort => format!("{m:02}/{d:02}/{:02}", y % 100),
            DateFmt::MdyNoPad => format!("{m}/{d}/{y}"),
            DateFmt::MdyDash => format!("{m:02}-{d:02}-{y}"),
            DateFmt::MonD => format!("{} {d}, {y}", &mon[..3]),
            DateFmt::MonthD => format!("{mon} {d}, {y}"),
            DateFmt::Iso => self.iso(),
            DateFmt::DMonY => format!("{d:02}-{}-{y}", &mon[..3]),
            DateFmt::MonDUpper => format!("{} {d:02} {y}", mon[..3].to_uppercase()),
        }
    }
}

/// How a layout prints dates (all common on US bills and receipts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateFmt {
    /// 03/14/2026
    Mdy,
    /// 03/14/26
    MdyShort,
    /// 3/14/2026
    MdyNoPad,
    /// 03-14-2026
    MdyDash,
    /// Mar 14, 2026
    MonD,
    /// March 14, 2026
    MonthD,
    /// 2026-03-14
    Iso,
    /// 14-Mar-2026
    DMonY,
    /// MAR 14 2026 (thermal receipts)
    MonDUpper,
}

/// How a layout prints amounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyFmt {
    /// $1,234.56
    Dollar,
    /// 1,234.56
    Plain,
    /// 1234.56
    Bare,
    /// $ 1,234.56
    DollarSpace,
    /// USD 1,234.56
    Usd,
}

fn grouped(mut n: i64) -> String {
    let mut parts = Vec::new();
    loop {
        if n < 1000 {
            parts.push(n.to_string());
            break;
        }
        parts.push(format!("{:03}", n % 1000));
        n /= 1000;
    }
    parts.reverse();
    parts.join(",")
}

/// `cents` as printed; negative amounts get a leading minus.
pub fn money(cents: i64, f: MoneyFmt) -> String {
    let neg = cents < 0;
    let c = cents.unsigned_abs();
    let whole = i64::try_from(c / 100).unwrap_or(0);
    let frac = c % 100;
    let body = match f {
        MoneyFmt::Bare => format!("{whole}.{frac:02}"),
        _ => format!("{}.{frac:02}", grouped(whole)),
    };
    let s = match f {
        MoneyFmt::Dollar => format!("${body}"),
        MoneyFmt::DollarSpace => format!("$ {body}"),
        MoneyFmt::Usd => format!("USD {body}"),
        MoneyFmt::Plain | MoneyFmt::Bare => body,
    };
    if neg { format!("-{s}") } else { s }
}

/// `cents` as a wire string for the truth files ("1234.56", "-3.10").
pub fn wire(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let c = cents.unsigned_abs();
    format!("{sign}{}.{:02}", c / 100, c % 100)
}

/// A quantity in thousandths as printed ("3", "2.5", "0.75", "12.345").
pub fn quantity(milli: i64) -> String {
    let whole = milli / 1000;
    let frac = milli % 1000;
    if frac == 0 {
        whole.to_string()
    } else if frac % 100 == 0 {
        format!("{whole}.{}", frac / 100)
    } else if frac % 10 == 0 {
        format!("{whole}.{:02}", frac / 10)
    } else {
        format!("{whole}.{frac:03}")
    }
}

/// A unit price in thousandths of a dollar: cents when whole cents
/// ("19.99" in `f`), else three places ("3.259", fuel).
pub fn rate(milli: i64, f: MoneyFmt) -> String {
    if milli % 10 == 0 {
        money(milli / 10, f)
    } else {
        let s = format!("{}.{:03}", milli / 1000, milli % 1000);
        match f {
            MoneyFmt::Dollar => format!("${s}"),
            MoneyFmt::DollarSpace => format!("$ {s}"),
            MoneyFmt::Usd => format!("USD {s}"),
            MoneyFmt::Plain | MoneyFmt::Bare => s,
        }
    }
}

/// `qty` (thousandths) × `rate` (thousandths of a dollar), rounded half up
/// to cents.
pub fn extend(qty_milli: i64, rate_milli: i64) -> i64 {
    (qty_milli * rate_milli + 5_000).div_euclid(10_000)
}

/// Thousandths as a decimal string ("12.345").
pub fn milli_wire(milli: i64) -> String {
    format!("{}.{:03}", milli / 1000, milli % 1000)
}

/// `base` × `bp` basis points, rounded half up to cents.
pub fn percent(base: i64, bp: i64) -> i64 {
    (base * bp + 5_000).div_euclid(10_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip_and_print() {
        let d = Date {
            y: 2026,
            m: 3,
            d: 4,
        };
        assert_eq!(Date::from_days(d.days()), d);
        assert_eq!(d.plus(30).iso(), "2026-04-03");
        assert_eq!(
            Date {
                y: 2024,
                m: 2,
                d: 28
            }
            .plus(1)
            .iso(),
            "2024-02-29"
        );
        assert_eq!(d.format(DateFmt::MonD), "Mar 4, 2026");
        assert_eq!(d.format(DateFmt::MdyShort), "03/04/26");
        assert_eq!(d.format(DateFmt::DMonY), "04-Mar-2026");
        assert_eq!(d.format(DateFmt::MonDUpper), "MAR 04 2026");
    }

    #[test]
    fn amounts_print_exactly() {
        assert_eq!(money(123_456, MoneyFmt::Dollar), "$1,234.56");
        assert_eq!(money(5, MoneyFmt::Plain), "0.05");
        assert_eq!(money(100_000_000, MoneyFmt::Bare), "1000000.00");
        assert_eq!(money(-1_000, MoneyFmt::Usd), "-USD 10.00");
        assert_eq!(wire(-310), "-3.10");
        assert_eq!(quantity(2500), "2.5");
        assert_eq!(quantity(12_345), "12.345");
        assert_eq!(rate(3259, MoneyFmt::Dollar), "$3.259");
        assert_eq!(rate(19_990, MoneyFmt::Dollar), "$19.99");
        assert_eq!(quantity(750), "0.75");
        assert_eq!(extend(2500, 19_990), 4998);
        assert_eq!(extend(12_345, 3259), 4023);
        assert_eq!(percent(10_000, 725), 725);
    }
}
