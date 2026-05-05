use std::collections::HashSet;
use std::sync::LazyLock;

use super::Language;

#[derive(Debug, Clone)]
pub struct Japanese {}
static JAPANESE_ABBREVIATIONS: LazyLock<HashSet<String>> = LazyLock::new(HashSet::new);

impl Language for Japanese {
    fn get_abbreviations(&self) -> &HashSet<String> {
        &JAPANESE_ABBREVIATIONS
    }
}

#[cfg(test)]
mod tests {
    use crate::languages::tests::run_language_tests;

    use super::*;

    #[test]
    fn test_segment() {
        run_language_tests(Japanese {}, "tests/ja.txt");
    }
}
