extern crate research_ledger_idempotency;

fn main() {
    let mut ids = Vec::new();
    assert!(research_ledger_idempotency::idempotency::record_once(&mut ids, 7));
    assert!(!research_ledger_idempotency::idempotency::record_once(&mut ids, 7));
    assert_eq!(ids, vec![7]);
}
