use super::Language;
use super::parse_word_list;
use std::collections::HashSet;
use std::sync::LazyLock;

#[derive(Debug, Clone)]
pub struct English {}

static ENGLISH_ABBREVIATIONS: LazyLock<HashSet<String>> =
    LazyLock::new(|| parse_word_list([include_str!("./abbrev/en.txt")]));

static ENGLISH_SENTENCE_STARTERS: LazyLock<HashSet<String>> =
    LazyLock::new(|| parse_word_list([include_str!("./starters/en.txt")]));

impl Language for English {
    fn get_abbreviations(&self) -> &HashSet<String> {
        &ENGLISH_ABBREVIATIONS
    }

    fn get_sentence_starters(&self) -> &HashSet<String> {
        &ENGLISH_SENTENCE_STARTERS
    }
}

#[cfg(test)]
mod tests {
    use crate::languages::tests::run_language_tests;

    use super::*;

    #[test]
    fn test_segment() {
        run_language_tests(English {}, "tests/en.txt");
    }
}
