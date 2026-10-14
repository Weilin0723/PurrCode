pub const DEFAULT_CURRENCY: &str = "USD";

#[cfg(test)]
mod tests {
    use super::DEFAULT_CURRENCY;

    #[test]
    fn configured_currency_is_nonempty() {
        assert!(!DEFAULT_CURRENCY.is_empty());
    }
}
