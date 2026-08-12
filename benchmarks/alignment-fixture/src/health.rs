//! Liveness.
//!
//! Reports that the process is up. It says nothing about whether the things it
//! depends on are reachable, which is the question anyone asking a health
//! endpoint is actually asking.

use crate::response::{self, Response};

pub fn health() -> Response {
    response::ok("{\"status\":\"up\"}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_answers() {
        assert_eq!(health().status, 200);
    }
}
