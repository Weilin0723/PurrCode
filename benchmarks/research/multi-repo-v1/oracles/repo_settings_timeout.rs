extern crate research_settings_timeout;

fn main() {
    assert_eq!(research_settings_timeout::timeout::timeout_ms(None), 30_000);
    assert_eq!(research_settings_timeout::timeout::timeout_ms(Some(5_000)), 5_000);
    assert_eq!(research_settings_timeout::timeout::timeout_ms(Some(0)), 0);
}
