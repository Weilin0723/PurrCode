extern crate research_scheduler_order;

fn main() {
    let mut items = vec![
        (5, "normal".to_string()),
        (10, "zeta".to_string()),
        (10, "urgent".to_string()),
    ];
    research_scheduler_order::order::ready_order(&mut items);
    assert_eq!(
        items,
        vec![
            (10, "urgent".to_string()),
            (10, "zeta".to_string()),
            (5, "normal".to_string()),
        ]
    );
}
