pub const DEFAULT_QUEUE: &str = "default";

#[cfg(test)]
mod tests {
    use super::DEFAULT_QUEUE;

    #[test]
    fn configured_queue_is_nonempty() {
        assert!(!DEFAULT_QUEUE.is_empty());
    }
}
