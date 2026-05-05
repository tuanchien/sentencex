use std::collections::HashSet;
use std::sync::LazyLock;

use super::Language;
use super::parse_word_list;

#[derive(Debug, Clone)]
pub struct French {}
static FRENCH_ABBREVIATIONS: LazyLock<HashSet<String>> =
    LazyLock::new(|| parse_word_list([include_str!("./abbrev/fr.txt")]));

impl Language for French {
    fn get_abbreviations(&self) -> &HashSet<String> {
        &FRENCH_ABBREVIATIONS
    }
}

#[cfg(test)]
mod tests {
    use crate::languages::tests::run_language_tests;

    use super::*;

    #[test]
    fn test_segment() {
        run_language_tests(French {}, "tests/fr.txt");
    }
}
