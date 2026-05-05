use super::Language;
use super::parse_word_list;
use std::collections::HashSet;
use std::sync::LazyLock;

#[derive(Debug, Clone)]
pub struct Punjabi {}

static ABBREVIATIONS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    parse_word_list([
        include_str!("./abbrev/pa.txt"),
        include_str!("./abbrev/en.txt"),
    ])
});
impl Language for Punjabi {
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
        run_language_tests(Punjabi {}, "tests/pa.txt");
    }
}
