pub fn is_even(value: i32) -> bool {
    value % 2 == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_even_values() {
        assert!(is_even(4));
    }
}
