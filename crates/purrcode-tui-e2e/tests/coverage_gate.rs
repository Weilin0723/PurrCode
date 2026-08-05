//! The gate that stops the coverage report from claiming tests that do not exist.
//!
//! `ui_actions::SCENARIOS` names a PTY test for each scenario it claims to cover.
//! Without this check that table is just a wish list: a scenario could reference
//! `tests/approval.rs::some_test` forever without anyone writing it, and
//! `purrcode ui-actions coverage` would report the action as covered.
//!
//! This reads the actual test sources and fails when a referenced test is absent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use purrcode_tui::{SCENARIOS, ScenarioKind};

fn tests_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// Test function names defined in each `tests/*.rs` file.
fn declared_tests() -> BTreeMap<String, Vec<String>> {
    let mut declared = BTreeMap::new();
    let directory = tests_directory();
    let entries = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.expect("read test directory entry");
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let file = format!(
            "tests/{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .expect("test file name")
        );
        declared.insert(file, test_names(&path));
    }
    declared
}

/// Extract `fn name(` for functions directly preceded by `#[test]`.
fn test_names(path: &Path) -> Vec<String> {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let mut names = Vec::new();
    let mut previous_was_test_attribute = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if previous_was_test_attribute {
            if let Some(rest) = trimmed.strip_prefix("fn ")
                && let Some(name) = rest.split('(').next()
            {
                names.push(name.trim().to_owned());
            }
            previous_was_test_attribute = trimmed.starts_with("#[");
            continue;
        }
        previous_was_test_attribute = trimmed == "#[test]";
    }
    names
}

#[test]
fn every_referenced_pty_test_exists() {
    let declared = declared_tests();
    let mut missing = Vec::new();
    for scenario in SCENARIOS {
        let Some(reference) = scenario.pty_test else {
            continue;
        };
        let Some((file, name)) = reference.split_once("::") else {
            missing.push(format!(
                "{}: malformed reference {reference:?}",
                scenario.id
            ));
            continue;
        };
        match declared.get(file) {
            Some(names) if names.iter().any(|declared| declared == name) => {}
            Some(_) => missing.push(format!("{}: {file} has no test named {name}", scenario.id)),
            None => missing.push(format!("{}: {file} does not exist", scenario.id)),
        }
    }
    assert!(
        missing.is_empty(),
        "the acceptance coverage report claims PTY tests that do not exist.\n\
         Either write the test or set `pty_test: None` for the scenario:\n{}",
        missing.join("\n")
    );
}

#[test]
fn every_pty_test_reference_is_well_formed() {
    for scenario in SCENARIOS {
        if let Some(reference) = scenario.pty_test {
            assert!(
                reference.starts_with("tests/") && reference.contains(".rs::"),
                "{}: {reference:?} must be `tests/<file>.rs::<test name>`",
                scenario.id
            );
        }
    }
}

/// A scenario the release gate calls critical must be proven by a real test.
#[test]
fn critical_scenarios_are_proven_by_a_real_test() {
    let unproven: Vec<&str> = SCENARIOS
        .iter()
        .filter(|scenario| scenario.critical && scenario.pty_test.is_none())
        .map(|scenario| scenario.id.as_str())
        .collect();
    assert!(
        unproven.is_empty(),
        "critical scenarios with no PTY test: {unproven:?}"
    );
}

/// Report what is not yet automated, so a gap is visible rather than silent.
#[test]
fn unautomated_scenarios_are_reported() {
    let gaps: Vec<String> = SCENARIOS
        .iter()
        .filter(|scenario| scenario.pty_test.is_none())
        .map(|scenario| {
            format!(
                "  {} [{}] {}",
                scenario.id,
                scenario.kind.label(),
                scenario.summary
            )
        })
        .collect();
    if !gaps.is_empty() {
        // Printed rather than asserted: these are declared, non-critical gaps, and
        // hiding them would be worse than a noisy pass.
        println!(
            "{} scenario(s) are declared but not yet automated:\n{}",
            gaps.len(),
            gaps.join("\n")
        );
    }
    // Every non-automated scenario must at least be on a real-terminal checklist,
    // otherwise nothing verifies it at all.
    let unverified: Vec<&str> = SCENARIOS
        .iter()
        .filter(|scenario| scenario.pty_test.is_none() && scenario.real_terminal_case.is_none())
        .map(|scenario| scenario.id.as_str())
        .collect();
    assert!(
        unverified.is_empty(),
        "scenarios with neither a PTY test nor a real-terminal case: {unverified:?}"
    );
}

#[test]
fn every_scenario_kind_is_exercised_somewhere() {
    for kind in [
        ScenarioKind::Smoke,
        ScenarioKind::Failure,
        ScenarioKind::Cancellation,
        ScenarioKind::Restart,
        ScenarioKind::Recovery,
    ] {
        assert!(
            SCENARIOS
                .iter()
                .any(|scenario| scenario.kind == kind && scenario.pty_test.is_some()),
            "no automated scenario of kind {}",
            kind.label()
        );
    }
}
