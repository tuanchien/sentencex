use std::collections::HashSet;
use std::sync::LazyLock;

use super::Language;
use super::parse_word_list;

#[derive(Debug, Clone)]
pub struct Tamil {}
static TAMIL_ABBREVIATIONS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    let vowel_signs = ["ா", "ி", "ீ", "ু", "ূ", "ে", "ে", "ৈ", "ও", "ো", "ৌ"];
    let vowels = ["அ", "ஆ", "இ", "ஈ", "உ", "ஊ", "எ", "ஏ", "ஐ", "ஒ", "ஓ", "ஔ"];
    let consonants = [
        "க", "ங", "ச", "ஞ", "ட", "ண", "த", "ந", "ப", "ம", "ய", "ர", "ல", "வ", "ழ", "ள", "ற", "ன",
    ];

    let mut abbreviations = parse_word_list([
        include_str!("./abbrev/ta.txt"),
        include_str!("./abbrev/en.txt"),
    ]);
    abbreviations.extend(vowels.iter().map(|&s| s.to_string()));
    abbreviations.extend(consonants.iter().map(|&s| s.to_string()));
    for consonant in &consonants {
        for vowel_sign in &vowel_signs {
            abbreviations.insert(format!("{}{}", consonant, vowel_sign));
        }
    }
    abbreviations
});

impl Language for Tamil {
    fn get_abbreviations(&self) -> &HashSet<String> {
        &TAMIL_ABBREVIATIONS
    }
}

#[cfg(test)]
mod tests {
    use crate::languages::tests::run_language_tests;

    use super::*;

    #[test]
    fn test_segment() {
        run_language_tests(Tamil {}, "tests/ta.txt");
    }
}
