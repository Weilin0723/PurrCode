extern crate research_scheduler_backoff;

fn main() {
    assert_eq!(research_scheduler_backoff::backoff::backoff_ms(10, 0), 10);
    assert_eq!(research_scheduler_backoff::backoff::backoff_ms(10, 1), 20);
    assert_eq!(research_scheduler_backoff::backoff::backoff_ms(10, 2), 40);
    assert_eq!(research_scheduler_backoff::backoff::backoff_ms(10, 4), 160);
    assert_eq!(research_scheduler_backoff::backoff::backoff_ms(10_000, 4), 30_000);
}
