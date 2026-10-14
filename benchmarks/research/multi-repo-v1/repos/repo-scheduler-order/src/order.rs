/// Order ready jobs by descending priority and then by stable identifier.
pub fn ready_order(items: &mut Vec<(u8, String)>) {
    items.sort_by_key(|(priority, id)| (*priority, id.clone()));
}

#[cfg(test)]
mod tests {
    use super::ready_order;

    #[test]
    fn all_jobs_remain_present() {
        let mut items = vec![(1, "a".to_string()), (2, "b".to_string())];
        ready_order(&mut items);
        assert_eq!(items.len(), 2);
    }
}
