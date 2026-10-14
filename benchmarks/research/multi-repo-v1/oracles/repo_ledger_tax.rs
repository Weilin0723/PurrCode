extern crate research_ledger_tax;

fn main() {
    assert_eq!(research_ledger_tax::tax::tax_cents(100, 1_000), 10);
    assert_eq!(research_ledger_tax::tax::tax_cents(105, 1_000), 11);
    assert_eq!(research_ledger_tax::tax::tax_cents(1, 1_000), 0);
}
