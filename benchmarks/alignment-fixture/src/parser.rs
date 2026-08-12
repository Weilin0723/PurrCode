//! Parsing `key=value` settings lines.

/// Parse `key=value` lines, ignoring blanks and `#` comments.
pub fn parse(input: &str) -> Vec<(String, String)> {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_pairs_and_skips_comments() {
        let parsed = parse("# a comment\nname = purrcode\n\nmode=build\n");
        assert_eq!(
            parsed,
            vec![
                ("name".to_owned(), "purrcode".to_owned()),
                ("mode".to_owned(), "build".to_owned()),
            ]
        );
    }
}
