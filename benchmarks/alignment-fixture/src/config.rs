//! Application configuration.

/// How long a request may take before it is abandoned, in seconds.
pub const DEFAULT_REQUEST_TIMEOUT_SECONDS: u64 = 5;

/// The number of items a page holds unless the caller says otherwise.
pub const DEFAULT_PAGE_SIZE: usize = 20;

/// The largest page a caller may ask for.
pub const MAXIMUM_PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub request_timeout_seconds: u64,
    pub page_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            request_timeout_seconds: DEFAULT_REQUEST_TIMEOUT_SECONDS,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_what_the_constants_say() {
        let config = Config::default();
        assert_eq!(
            config.request_timeout_seconds,
            DEFAULT_REQUEST_TIMEOUT_SECONDS
        );
        assert_eq!(config.page_size, DEFAULT_PAGE_SIZE);
    }
}
