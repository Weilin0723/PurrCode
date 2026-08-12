//! Accounts, and the links between them.
//!
//! Links live in memory and are lost on restart. The `migrations/` directory
//! holds the schema this would need to persist them.

use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Accounts {
    /// account id → linked provider identity. In memory only.
    links: BTreeMap<String, String>,
}

impl Accounts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn link(&mut self, account: &str, provider_identity: &str) {
        self.links
            .insert(account.to_owned(), provider_identity.to_owned());
    }

    pub fn linked(&self, account: &str) -> Option<&str> {
        self.links.get(account).map(String::as_str)
    }

    pub fn unlink(&mut self, account: &str) {
        self.links.remove(account);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_round_trips_within_one_process() {
        let mut accounts = Accounts::new();
        accounts.link("a-1", "github:octocat");
        assert_eq!(accounts.linked("a-1"), Some("github:octocat"));
        accounts.unlink("a-1");
        assert_eq!(accounts.linked("a-1"), None);
    }
}
