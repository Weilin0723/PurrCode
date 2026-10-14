use purrcode_alignment_fixture::pagination;

fn main() {
    assert_eq!(pagination::page(&[1, 2, 3], 0, 3), vec![1, 2, 3]);
    assert_eq!(pagination::page(&[1, 2, 3], 1, 2), vec![2, 3]);
    assert!(pagination::page(&[1, 2, 3], 3, 10).is_empty());
    assert!(pagination::page(&[1, 2, 3], 0, 0).is_empty());
    assert_eq!(pagination::page_count(21, 10), 3);
}
