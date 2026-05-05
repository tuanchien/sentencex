use super::Language;
use super::parse_word_list;
use std::collections::HashSet;
use std::sync::LazyLock;

#[derive(Debug, Clone)]
pub struct Polish {}

static ABBREVIATIONS: LazyLock<HashSet<String>> =
    LazyLock::new(|| parse_word_list([include_str!("./abbrev/pl.txt")]));
impl Language for Polish {
    fn get_abbreviations(&self) -> &HashSet<String> {
        &ABBREVIATIONS
    }
}

#[cfg(test)]
mod tests {
    use crate::languages::tests::run_language_tests;

    use super::*;

    #[test]
    fn test_segment() {
        run_language_tests(Polish {}, "tests/pl.txt");
    }
}
