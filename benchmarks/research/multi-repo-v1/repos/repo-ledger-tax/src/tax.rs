/// Convert a basis-point tax rate into whole cents.
pub fn tax_cents(gross_cents: u64, rate_bps: u64) -> u64 {
    gross_cents.saturating_mul(rate_bps) / 10_000
}

#[cfg(test)]
mod tests {
    use super::tax_cents;

    #[test]
    fn exact_rate_is_preserved() {
        assert_eq!(tax_cents(100, 1_000), 10);
        assert_eq!(tax_cents(0, 1_000), 0);
    }
}
