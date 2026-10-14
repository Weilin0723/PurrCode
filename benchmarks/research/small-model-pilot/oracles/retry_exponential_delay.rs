use purrcode_alignment_fixture::retry;

fn main() {
    assert_eq!(retry::MAX_RETRIES, 3);
    assert_eq!(retry::delay_for(0), 100);
    assert_eq!(retry::delay_for(1), 200);
    assert_eq!(retry::delay_for(2), 400);
    assert_eq!(retry::delay_for(3), 800);
    assert_eq!(retry::delay_for(4), 1600);
    assert_eq!(retry::delay_for(5), 1600);

    let mut attempts = 0;
    let result: Result<(), &str> = retry::with_retries(|| {
        attempts += 1;
        Err("no")
    });
    assert!(result.is_err());
    assert_eq!(attempts, retry::MAX_RETRIES + 1);

    let mut first_success_attempts = 0;
    let result: Result<u32, &str> = retry::with_retries(|| {
        first_success_attempts += 1;
        Ok(7)
    });
    assert_eq!(result, Ok(7));
    assert_eq!(first_success_attempts, 1);
}
