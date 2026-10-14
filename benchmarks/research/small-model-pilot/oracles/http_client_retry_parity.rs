use purrcode_alignment_fixture::{config, http};

fn main() {
    let accounts = http::accounts_client("https://accounts.invalid");
    let reporting = http::reporting_client("https://reports.invalid");
    assert_eq!(accounts.retries, 3);
    assert_eq!(reporting.retries, 3);
    assert_eq!(accounts.timeout_seconds, config::DEFAULT_REQUEST_TIMEOUT_SECONDS);
    assert_eq!(reporting.timeout_seconds, config::DEFAULT_REQUEST_TIMEOUT_SECONDS);
    assert_eq!(accounts.user_agent, "purrcode-fixture/0.1");
    assert_eq!(reporting.user_agent, "purrcode-fixture/0.1");
    assert_eq!(accounts.base_url, "https://accounts.invalid");
    assert_eq!(reporting.base_url, "https://reports.invalid");
}
