use purrcode_alignment_fixture::{reporter, scheduler};

fn main() {
    let mut jobs = scheduler::schedule(&["Nightly", "Weekly"]);
    assert_eq!(jobs[0].id, "job-nightly");
    assert_eq!(jobs[1].id, "job-weekly");

    scheduler::complete(&mut jobs, "job-nightly");
    assert!(jobs[0].done);
    assert!(!jobs[1].done);

    let completed = reporter::completed(&jobs, &["NIGHTLY", "weekly", "missing"]);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].name, "Nightly");
    assert_eq!(reporter::completed(&jobs, &["WEEKLY", "missing"]).len(), 0);
    assert_eq!(reporter::summary(&jobs, &["NIGHTLY", "weekly", "missing"]), "1 of 2 job(s) finished");
}
