use std::collections::HashSet;
use std::sync::LazyLock;

use super::Language;

#[derive(Debug, Clone)]
pub struct Bulgarian {}
static BULGARIAN_ABBREVIATIONS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    include_str!("./abbrev/bg.txt")
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.starts_with("//") && !line.is_empty())
        .collect()
});

impl Language for Bulgarian {
    fn get_abbreviations(&self) -> &HashSet<String> {
        &BULGARIAN_ABBREVIATIONS
    }
}

#[cfg(test)]
mod tests {
    use crate::languages::tests::run_language_tests;

    use super::*;

    #[test]
    fn test_segment() {
        run_language_tests(Bulgarian {}, "tests/bg.txt");
    }
}
