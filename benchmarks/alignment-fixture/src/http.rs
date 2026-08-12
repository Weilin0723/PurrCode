//! Talking to the two upstream services.
//!
//! The client setup is written twice, once per service. They have already
//! drifted: one retries and one does not.

use crate::config::DEFAULT_REQUEST_TIMEOUT_SECONDS;

#[derive(Clone, Debug, PartialEq)]
pub struct Client {
    pub base_url: String,
    pub timeout_seconds: u64,
    pub user_agent: String,
    pub retries: u32,
}

/// The client the accounts service uses.
pub fn accounts_client(base_url: &str) -> Client {
    Client {
        base_url: base_url.to_owned(),
        timeout_seconds: DEFAULT_REQUEST_TIMEOUT_SECONDS,
        user_agent: "purrcode-fixture/0.1".to_owned(),
        retries: 3,
    }
}

/// The client the reporting service uses.
pub fn reporting_client(base_url: &str) -> Client {
    Client {
        base_url: base_url.to_owned(),
        timeout_seconds: DEFAULT_REQUEST_TIMEOUT_SECONDS,
        user_agent: "purrcode-fixture/0.1".to_owned(),
        retries: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_clients_carry_the_same_identity() {
        assert_eq!(
            accounts_client("https://a.invalid").user_agent,
            reporting_client("https://b.invalid").user_agent
        );
    }
}
