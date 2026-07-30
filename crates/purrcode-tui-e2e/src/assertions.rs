//! Screen assertions with useful failure messages.
//!
//! A failing acceptance test must say what the user would have seen, otherwise
//! diagnosing it means re-running by hand. Every assertion here prints the whole
//! screen on failure.

use crate::screen::Screen;

/// Assert that every needle is visible on some row.
#[track_caller]
pub fn assert_visible(screen: &Screen, needles: &[&str]) {
    let missing: Vec<&str> = needles
        .iter()
        .copied()
        .filter(|needle| !screen.contains(needle))
        .collect();
    assert!(
        missing.is_empty(),
        "expected to see {missing:?} but the screen was:\n{}",
        screen.text()
    );
}

/// Assert that no needle appears anywhere on screen.
#[track_caller]
pub fn assert_absent(screen: &Screen, needles: &[&str]) {
    let present: Vec<&str> = needles
        .iter()
        .copied()
        .filter(|needle| screen.contains(needle))
        .collect();
    assert!(
        present.is_empty(),
        "expected {present:?} to be absent but the screen was:\n{}",
        screen.text()
    );
}

/// Assert that a sentence is readable, allowing it to wrap across rows.
#[track_caller]
pub fn assert_readable(screen: &Screen, sentence: &str) {
    assert!(
        screen.contains_wrapped(sentence),
        "expected the screen to read {sentence:?} but it was:\n{}",
        screen.text()
    );
}

/// Assert that a sentence reads within a column slice of the screen.
#[track_caller]
pub fn assert_readable_in_column(screen: &Screen, from: u16, to: u16, sentence: &str) {
    assert!(
        screen.contains_in_column(from, to, sentence),
        "expected columns {from}..{to} to read {sentence:?} but the screen was:\n{}",
        screen.text()
    );
}

/// Assert that the interface drew nothing past the terminal's right edge.
#[track_caller]
pub fn assert_no_overflow(screen: &Screen) {
    assert!(
        screen.fits_width(),
        "the interface drew past {} columns:\n{}",
        screen.width(),
        screen.text()
    );
}

/// Assert that no row is a bare status word with nothing actionable near it.
///
/// Guards the product rule that an error must never appear without a recovery
/// path: if the screen says something failed, it must also offer a next step.
#[track_caller]
pub fn assert_failure_offers_recovery(screen: &Screen, failure: &str, recovery: &[&str]) {
    assert!(
        screen.contains(failure),
        "expected the failure {failure:?} to be visible:\n{}",
        screen.text()
    );
    let offered = recovery.iter().any(|option| screen.contains(option));
    assert!(
        offered,
        "the screen reported {failure:?} without offering any of {recovery:?}:\n{}",
        screen.text()
    );
}

/// Assert that no secret-shaped value reached the screen.
#[track_caller]
pub fn assert_no_secret(screen: &Screen, secrets: &[&str]) {
    for secret in secrets {
        assert!(
            !screen.contains(secret),
            "a secret-shaped value reached the terminal:\n{}",
            screen.text()
        );
    }
    // Common credential prefixes, in case a test forgets to list its own.
    for prefix in ["sk-live-", "sk-test-", "ghp_", "AKIA"] {
        assert!(
            !screen.contains(prefix),
            "a value starting with {prefix:?} reached the terminal:\n{}",
            screen.text()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(text: &str, width: u16) -> Screen {
        let height = text.lines().count().max(1) as u16;
        let mut parser = vt100::Parser::new(height, width, 0);
        parser.process(text.replace('\n', "\r\n").as_bytes());
        Screen::from_parser(&parser)
    }

    #[test]
    fn visible_assertions_pass_when_everything_is_present() {
        assert_visible(
            &screen("Approval required\nA Approve", 40),
            &["Approval required", "A Approve"],
        );
    }

    #[test]
    #[should_panic(expected = "expected to see")]
    fn visible_assertions_report_what_was_missing() {
        assert_visible(&screen("only this", 40), &["something else"]);
    }

    #[test]
    #[should_panic(expected = "the screen was")]
    fn visible_failures_include_the_screen() {
        assert_visible(&screen("actual content", 40), &["missing"]);
    }

    #[test]
    fn absent_assertions_pass_when_nothing_matches() {
        assert_absent(&screen("safe output", 40), &["Approve", "digest"]);
    }

    #[test]
    #[should_panic(expected = "to be absent")]
    fn absent_assertions_fail_when_something_matches() {
        assert_absent(&screen("A Approve exact action", 40), &["Approve"]);
    }

    #[test]
    fn readable_assertions_tolerate_wrapping() {
        assert_readable(
            &screen("a sentence that\nwraps across rows", 20),
            "a sentence that wraps across rows",
        );
    }

    #[test]
    fn column_scoped_assertions_ignore_neighbouring_columns() {
        let screen = screen("left | wrapped\nleft | sentence", 20);
        assert_readable_in_column(&screen, 7, 20, "wrapped sentence");
    }

    #[test]
    fn overflow_assertions_pass_for_a_well_behaved_screen() {
        assert_no_overflow(&screen("short line", 40));
    }

    #[test]
    fn a_failure_without_a_recovery_option_is_rejected() {
        let result = std::panic::catch_unwind(|| {
            assert_failure_offers_recovery(
                &screen("Validation failed", 40),
                "Validation failed",
                &["Roll back", "Resume"],
            );
        });
        assert!(
            result.is_err(),
            "a failure with no offered recovery must not pass"
        );
    }

    #[test]
    fn a_failure_with_a_recovery_option_passes() {
        assert_failure_offers_recovery(
            &screen("Validation failed\nCtrl+R Roll back", 40),
            "Validation failed",
            &["Roll back", "Resume"],
        );
    }

    #[test]
    fn secret_assertions_catch_listed_and_common_shapes() {
        assert_no_secret(&screen("keychain:default", 40), &["my-secret-value"]);

        for leaked in ["sk-live-abcdef", "ghp_aaaaaaaaaaaa", "AKIAIOSFODNN7EXAMPLE"] {
            let result = std::panic::catch_unwind(move || {
                assert_no_secret(&screen(leaked, 60), &[]);
            });
            assert!(result.is_err(), "{leaked} should have been caught");
        }
    }

    #[test]
    fn a_listed_secret_is_caught_even_with_an_unfamiliar_shape() {
        let result = std::panic::catch_unwind(|| {
            assert_no_secret(&screen("token=zzz-custom-shape", 40), &["zzz-custom-shape"]);
        });
        assert!(result.is_err());
    }
}
