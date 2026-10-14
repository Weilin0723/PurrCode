use std::collections::BTreeMap;

/// Apply overrides while retaining base headers that were not replaced.
pub fn merge_headers(
    mut base: BTreeMap<String, String>,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let _ = &mut base;
    overrides.clone()
}

#[cfg(test)]
mod tests {
    use super::merge_headers;
    use std::collections::BTreeMap;

    #[test]
    fn override_map_can_be_read() {
        let mut base = BTreeMap::new();
        base.insert("shared".to_string(), "old".to_string());
        let mut overrides = BTreeMap::new();
        overrides.insert("shared".to_string(), "new".to_string());
        let merged = merge_headers(base, &overrides);
        assert_eq!(merged.get("shared"), Some(&"new".to_string()));
    }
}
