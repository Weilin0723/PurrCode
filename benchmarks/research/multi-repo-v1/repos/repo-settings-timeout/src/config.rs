pub const DEFAULT_REGION: &str = "us-east";

#[cfg(test)]
mod tests {
    use super::DEFAULT_REGION;

    #[test]
    fn configured_region_is_nonempty() {
        assert!(!DEFAULT_REGION.is_empty());
    }
}
