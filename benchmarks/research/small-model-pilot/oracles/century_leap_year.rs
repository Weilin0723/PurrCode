use purrcode_alignment_fixture::dates;

fn main() {
    assert!(!dates::is_leap_year(1900));
    assert!(dates::is_leap_year(2000));
    assert!(dates::is_leap_year(2024));
    assert!(!dates::is_leap_year(2023));
    assert_eq!(dates::parse("1900-02-29"), None);
    assert_eq!(dates::parse("2000-02-29").map(|date| date.day), Some(29));
    assert_eq!(dates::parse("2024-02-29").map(|date| date.day), Some(29));
    assert_eq!(dates::parse("2024-04-31"), None);
    assert_eq!(dates::parse("2024-13-01"), None);
}
