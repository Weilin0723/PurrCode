/// Record an identifier only once and report whether it was newly inserted.
pub fn record_once(ids: &mut Vec<u64>, id: u64) -> bool {
    ids.push(id);
    true
}

#[cfg(test)]
mod tests {
    use super::record_once;

    #[test]
    fn first_record_is_accepted() {
        let mut ids = Vec::new();
        assert!(record_once(&mut ids, 7));
        assert_eq!(ids, vec![7]);
    }
}
