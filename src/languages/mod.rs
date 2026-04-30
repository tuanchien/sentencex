mod am;
mod ar;
mod bg;
mod bn;
mod ca;
mod da;
mod de;
mod el;
mod en;
mod es;
mod fallbacks;
mod fi;
mod fr;
mod gu;
mod hi;
mod hy;
mod it;
mod ja;
mod kk;
mod kn;
mod language;
mod list_markers;
mod ml;
mod mr;
mod my;
mod nl;
mod pa;
mod pl;
mod pt;
mod ru;
mod sk;
mod ta;
mod te;
mod uk;

pub use am::Amharic;
pub use ar::Arabic;
pub use bg::Bulgarian;
pub use bn::Bengali;
pub use ca::Catalan;
pub use da::Danish;
pub use de::German;
pub use el::Greek;
pub use en::English;
pub use es::Spanish;
pub use fallbacks::get_fallbacks;
pub use fi::Finnish;
pub use fr::French;
pub use gu::Gujarati;
pub use hi::Hindi;
pub use hy::Armenian;
pub use it::Italian;
pub use ja::Japanese;
pub use kk::Kazakh;
pub use kn::Kannada;
pub use language::Language;
pub use ml::Malayalam;
pub use mr::Marathi;
pub use my::Burmese;
pub use nl::Dutch;
pub use pa::Punjabi;
pub use pl::Polish;
pub use pt::Portuguese;
pub use ru::Russian;
pub use sk::Slovak;
pub use ta::Tamil;
pub use te::Telugu;
pub use uk::Ukrainian;

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    pub fn run_language_tests<T: Language>(language: T, test_file: &str) {
        let raw = fs::read_to_string(test_file).expect("Failed to read test file");
        // Normalise line endings up-front so CRLF-checkout fixtures on Windows
        // behave identically to LF on Linux/macOS. Without this, segments would
        // contain stray `\r` characters that wouldn't match `\n`-escaped
        // expected values, and the `\n` escape decoding below would be
        // inconsistent across platforms.
        let content = raw.replace("\r\n", "\n");
        let test_cases: Vec<&str> = content.split("===").collect();

        for case in test_cases {
            // Strip comment-only lines (those whose first non-whitespace char
            // is `#`) at the line level. This lets fixture files include
            // section headers and structural comments anywhere — not just at
            // the very top of the file — without silently skipping cases that
            // happen to share a chunk with leading comments.
            let cleaned: String = case
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n");
            let cleaned = cleaned.trim();
            if cleaned.is_empty() {
                continue;
            }
            let parts: Vec<&str> = cleaned
                .split("---")
                .map(|part| part.trim())
                .filter(|part| !part.is_empty())
                .collect();
            if parts.len() != 2 {
                continue; // Skip malformed test cases
            }

            // Both input and expected support `\n` (literal backslash-n) as
            // an escape for a real newline. This lets a fixture express an
            // input containing newlines on a single line of source text, or
            // expand an expected segment across an embedded newline.
            //
            // Comparison normalises whitespace on both sides — any run of
            // whitespace (spaces, tabs, newlines) collapses to a single
            // space — so a single segment that wraps across lines in the
            // input can be written as a single line of expected text without
            // needing escapes at all. This is sound because the segmenter
            // only ever returns slices of the input; it never invents or
            // alters whitespace inside a segment, so a whitespace difference
            // in a segment would have to come from a boundary placement
            // change, which is also detectable via segment count/ordering.
            let decode = |s: &str| s.replace("\\n", "\n");
            let normalise = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");

            let input = decode(parts[0]);
            let expected: Vec<String> = parts[1]
                .lines()
                .map(decode)
                .map(|s| normalise(&s))
                .filter(|s| !s.is_empty())
                .collect();
            let result = language.segment(&input);
            let actual: Vec<String> = result
                .iter()
                .map(|item| normalise(item))
                .filter(|s| !s.is_empty())
                .collect();

            assert_eq!(actual, expected, "Failed for input: \n{}", input);
        }
    }

    #[test]
    fn run_language_tests_handles_crlf_fixtures() {
        // Build a fixture with CRLF endings, write it to a temp file, and
        // confirm the harness parses it identically to LF. Guards against
        // future changes that might re-introduce stray `\r` in segments or
        // expected values.
        let lf_content = "\
Hello world. This is a test.
---
Hello world.
This is a test.
===
Multi line sentence
we should not split this
---
Multi line sentence we should not split this
===
";
        let crlf_content = lf_content.replace('\n', "\r\n");

        let dir = std::env::temp_dir();
        let path = dir.join("sentencex_crlf_fixture_test.txt");
        fs::write(&path, crlf_content.as_bytes()).unwrap();

        run_language_tests(English {}, path.to_str().unwrap());

        let _ = fs::remove_file(&path);
    }
}
