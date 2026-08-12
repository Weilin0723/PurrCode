//! Slicing a list into pages.

/// The slice of `items` for one page.
///
/// Off by one at the end: the last item of a page is dropped.
pub fn page<T: Clone>(items: &[T], offset: usize, limit: usize) -> Vec<T> {
    let start = offset.min(items.len());
    let end = (start + limit).min(items.len());
    if end <= start {
        return Vec::new();
    }
    items[start..end - 1].to_vec()
}

/// How many pages `total` items make at `limit` per page.
pub fn page_count(total: usize, limit: usize) -> usize {
    if limit == 0 {
        return 0;
    }
    total.div_ceil(limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_list_has_no_pages() {
        let empty: Vec<u32> = Vec::new();
        assert!(page(&empty, 0, 10).is_empty());
        assert_eq!(page_count(0, 10), 0);
    }

    #[test]
    fn page_count_rounds_up() {
        assert_eq!(page_count(21, 10), 3);
    }
}
