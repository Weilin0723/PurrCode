pub const MAX_BACKOFF_MS: u64 = 30_000;

/// Calculate a bounded retry delay.
pub fn backoff_ms(base_ms: u64, attempt: u32) -> u64 {
    base_ms.saturating_mul(attempt as u64)
}

#[cfg(test)]
mod tests {
    use super::{backoff_ms, MAX_BACKOFF_MS};

    #[test]
    fn delay_stays_within_the_cap() {
        assert!(backoff_ms(10, 2) <= MAX_BACKOFF_MS);
    }
}
