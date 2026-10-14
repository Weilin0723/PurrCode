pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Select the configured timeout, falling back to the service default.
pub fn timeout_ms(configured: Option<u64>) -> u64 {
    configured.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::timeout_ms;

    #[test]
    fn explicit_timeout_is_preserved() {
        assert_eq!(timeout_ms(Some(5_000)), 5_000);
    }
}
