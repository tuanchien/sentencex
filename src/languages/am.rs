use std::collections::HashSet;
use std::sync::LazyLock;

use super::Language;
use super::parse_word_list;

#[derive(Debug, Clone)]
pub struct Amharic {}
// The previous loader chained `abbrev/am.txt` with itself; HashSet dedup makes
// that equivalent to loading the file once.
static AMHARIC_ABBREVIATIONS: LazyLock<HashSet<String>> =
    LazyLock::new(|| parse_word_list([include_str!("./abbrev/am.txt")]));

impl Language for Amharic {
    fn get_abbreviations(&self) -> &HashSet<String> {
        &AMHARIC_ABBREVIATIONS
    }
}

#[cfg(test)]
mod tests {
    use crate::languages::tests::run_language_tests;

    use super::*;

    #[test]
    fn test_segment() {
        run_language_tests(Amharic {}, "tests/am.txt");
    }
}
