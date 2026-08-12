//! Retrying a fallible operation.

/// How many times an operation is retried before giving up.
pub const MAX_RETRIES: u32 = 3;

/// How long to wait before attempt `attempt`, in milliseconds.
///
/// A flat delay. Every caller waits the same amount however busy the far end
/// is, which is the behaviour a backoff would replace.
pub fn delay_for(_attempt: u32) -> u64 {
    100
}

/// Run `operation`, retrying up to [`MAX_RETRIES`] times.
pub fn with_retries<T, E>(mut operation: impl FnMut() -> Result<T, E>) -> Result<T, E> {
    let mut attempt = 0;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt >= MAX_RETRIES => return Err(error),
            Err(_) => attempt += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_gives_up_after_the_maximum() {
        let mut attempts = 0;
        let result: Result<(), &str> = with_retries(|| {
            attempts += 1;
            Err("no")
        });
        assert!(result.is_err());
        assert_eq!(attempts, MAX_RETRIES + 1);
    }

    #[test]
    fn a_first_time_success_does_not_retry() {
        let mut attempts = 0;
        let result: Result<u32, &str> = with_retries(|| {
            attempts += 1;
            Ok(7)
        });
        assert_eq!(result, Ok(7));
        assert_eq!(attempts, 1);
    }
}
