//! Scheduling jobs.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// The id the scheduler assigns, lower-cased with a `job-` prefix.
    pub id: String,
    pub name: String,
    pub done: bool,
}

/// The id this scheduler gives a job.
pub fn job_id(name: &str) -> String {
    format!("job-{}", name.to_lowercase())
}

pub fn schedule(names: &[&str]) -> Vec<Job> {
    names
        .iter()
        .map(|name| Job {
            id: job_id(name),
            name: (*name).to_owned(),
            done: false,
        })
        .collect()
}

pub fn complete(jobs: &mut [Job], id: &str) {
    for job in jobs.iter_mut().filter(|job| job.id == id) {
        job.done = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduling_assigns_an_id_per_job() {
        let jobs = schedule(&["Nightly", "Weekly"]);
        assert_eq!(jobs[0].id, "job-nightly");
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn completing_marks_exactly_that_job() {
        let mut jobs = schedule(&["Nightly", "Weekly"]);
        complete(&mut jobs, "job-nightly");
        assert!(jobs[0].done);
        assert!(!jobs[1].done);
    }
}
