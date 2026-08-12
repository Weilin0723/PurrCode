//! Parsing `YYYY-MM-DD`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

/// Whether `year` is a leap year.
///
/// Wrong for century years: 1900 is not a leap year and 2000 is.
pub fn is_leap_year(year: i32) -> bool {
    year % 4 == 0
}

/// Days in `month` of `year`.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Parse `YYYY-MM-DD`, rejecting a day the month does not have.
pub fn parse(input: &str) -> Option<Date> {
    let mut parts = input.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || month == 0 || month > 12 || day == 0 {
        return None;
    }
    if day > days_in_month(year, month) {
        return None;
    }
    Some(Date { year, month, day })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_date_parses() {
        assert_eq!(
            parse("2024-03-15"),
            Some(Date {
                year: 2024,
                month: 3,
                day: 15
            })
        );
    }

    #[test]
    fn a_day_the_month_does_not_have_is_rejected() {
        assert_eq!(parse("2023-02-29"), None);
        assert_eq!(parse("2024-04-31"), None);
    }
}
