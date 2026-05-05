use super::Language;
use super::parse_word_list;
use std::collections::HashSet;
use std::sync::LazyLock;

#[derive(Debug, Clone)]
pub struct Kannada {}

static ABBREVIATIONS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    parse_word_list([
        include_str!("./abbrev/kn.txt"),
        include_str!("./abbrev/en.txt"),
    ])
});
impl Language for Kannada {
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
        run_language_tests(Kannada {}, "tests/kn.txt");
    }
}
