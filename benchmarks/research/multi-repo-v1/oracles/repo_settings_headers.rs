extern crate research_settings_headers;

use std::collections::BTreeMap;

fn main() {
    let mut base = BTreeMap::new();
    base.insert("keep".to_string(), "yes".to_string());
    base.insert("shared".to_string(), "old".to_string());
    let mut overrides = BTreeMap::new();
    overrides.insert("shared".to_string(), "new".to_string());
    overrides.insert("added".to_string(), "value".to_string());
    let merged = research_settings_headers::headers::merge_headers(base, &overrides);
    assert_eq!(merged.get("keep"), Some(&"yes".to_string()));
    assert_eq!(merged.get("shared"), Some(&"new".to_string()));
    assert_eq!(merged.get("added"), Some(&"value".to_string()));
}
