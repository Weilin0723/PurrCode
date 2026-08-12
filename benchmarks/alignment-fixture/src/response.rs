//! Building the JSON body every endpoint returns.
//!
//! The shape here is the public API. Tests cover the builder's inputs; they do
//! not currently pin the key names, so a rename would pass them.

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// A successful response carrying `data`.
pub fn ok(data: &str) -> Response {
    Response {
        status: 200,
        body: format!("{{\"ok\":true,\"data\":{data}}}"),
    }
}

/// A paged response.
pub fn paged(data: &str, total: usize, offset: usize, limit: usize) -> Response {
    Response {
        status: 200,
        body: format!(
            "{{\"ok\":true,\"data\":{data},\"total\":{total},\"offset\":{offset},\"limit\":{limit}}}"
        ),
    }
}

/// A failure carrying `message`.
pub fn error(status: u16, message: &str) -> Response {
    Response {
        status,
        body: format!("{{\"ok\":false,\"error\":\"{message}\"}}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_success_carries_its_data() {
        assert_eq!(ok("[1,2]").status, 200);
        assert!(ok("[1,2]").body.contains("[1,2]"));
    }

    #[test]
    fn a_failure_carries_its_message() {
        assert_eq!(error(404, "not found").status, 404);
        assert!(error(404, "not found").body.contains("not found"));
    }
}
