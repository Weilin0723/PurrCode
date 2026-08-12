//! Reporting on scheduled jobs.
//!
//! The lookup key here does not match what `scheduler` assigns, so a completed
//! job is never found and the report always says nothing finished.

use crate::scheduler::Job;

/// The id the reporter looks a job up by.
pub fn lookup_id(name: &str) -> String {
    format!("JOB_{}", name.to_uppercase())
}

/// The jobs the report considers finished.
pub fn completed<'a>(jobs: &'a [Job], names: &[&str]) -> Vec<&'a Job> {
    names
        .iter()
        .filter_map(|name| {
            let id = lookup_id(name);
            jobs.iter().find(|job| job.id == id && job.done)
        })
        .collect()
}

pub fn summary(jobs: &[Job], names: &[&str]) -> String {
    format!(
        "{} of {} job(s) finished",
        completed(jobs, names).len(),
        jobs.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler;

    #[test]
    fn the_summary_counts_the_jobs_it_was_given() {
        let jobs = scheduler::schedule(&["Nightly"]);
        assert!(summary(&jobs, &["Nightly"]).contains("of 1 job(s)"));
    }
}
