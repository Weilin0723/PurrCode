//! The item list endpoint.

use crate::pagination;
use crate::response::{self, Response};

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: u64,
    pub name: String,
}

/// Every item, in id order. Unpaged: the endpoint returns the lot.
pub fn list(items: &[Item]) -> Response {
    let rendered = items
        .iter()
        .map(|item| format!("{{\"id\":{},\"name\":\"{}\"}}", item.id, item.name))
        .collect::<Vec<_>>()
        .join(",");
    response::ok(&format!("[{rendered}]"))
}

/// One page of items, for callers that already know the offsets.
pub fn page(items: &[Item], offset: usize, limit: usize) -> Vec<Item> {
    pagination::page(items, offset, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Item> {
        (1..=3)
            .map(|id| Item {
                id,
                name: format!("item-{id}"),
            })
            .collect()
    }

    #[test]
    fn the_list_returns_every_item() {
        let response = list(&items());
        assert_eq!(response.status, 200);
        assert!(response.body.contains("item-3"));
    }
}
