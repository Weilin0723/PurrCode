//! The timeout test that fails intermittently in CI.
//!
//! It asserts something real — that the work finishes inside the configured
//! budget — and measures it against a wall clock with no slack, on a machine
//! that is also running everything else. Weakening the assertion would make it
//! pass and stop it meaning anything.

use std::time::{Duration, Instant};

use purrcode_alignment_fixture::config::DEFAULT_REQUEST_TIMEOUT_SECONDS;

fn work() {
    std::thread::sleep(Duration::from_millis(5));
}

#[test]
fn work_finishes_inside_the_request_timeout() {
    let budget = Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECONDS);
    let started = Instant::now();
    work();
    assert!(
        started.elapsed() < budget,
        "the work took {:?}, past the {budget:?} budget",
        started.elapsed()
    );
}

#[test]
fn the_deadline_is_computed_from_the_start_not_from_each_step() {
    let budget = Duration::from_millis(50);
    let started = Instant::now();
    for _ in 0..3 {
        work();
        assert!(
            started.elapsed() < budget,
            "cumulative time {:?} exceeded {budget:?}",
            started.elapsed()
        );
    }
}
