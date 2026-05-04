use regex::Regex;
use std::sync::LazyLock;

use crate::SentenceBoundary;
use crate::constants::EMAIL_REGEX;
use crate::constants::EXCLAMATION_WORDS;
use crate::constants::GLOBAL_SENTENCE_TERMINATORS;
use crate::constants::PARENS_REGEX;
use crate::constants::QUOTE_CLOSERS_BY_LEN;
use crate::constants::QUOTE_PAIRS;
use crate::constants::QUOTES_REGEX;
use crate::constants::SPACE_AFTER_SEPARATOR;

static DEFAULT_SENTENCE_BREAK_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // The leading `\.(?:[ \t]+\.){2,}` alternative coalesces space-separated dot
    // runs of three or more (`. . .`, `. . . .`, ...) into one match. Two-dot
    // forms like `. .` are intentionally excluded so a real period followed by
    // a leading-ellipsis (`raak. ...en`) is not eaten as `. .`. `[ \t]` (not
    // `\s`) avoids swallowing newlines and breaking paragraph splits. The
    // `[!?…]` branch coalesces spaced runs of two or more, mixed or homogeneous
    // (`! !`, `? ? ?`, `! ?`, `… !`, ...); it uses `+` rather than `{2,}`
    // because there is no convention of a sentence beginning with `!!!` or
    // `???`, so the leading-ellipsis ambiguity that motivates the dot rule
    // does not apply. Leftmost-first alternation requires these branches to
    // precede the class.
    let pattern = format!(
        r"\.(?:[ \t]+\.){{2,}}|[!?…](?:[ \t]+[!?…])+|[{}]+",
        GLOBAL_SENTENCE_TERMINATORS.join("")
    );
    Regex::new(&pattern).unwrap()
});

static CONTINUE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-z]").unwrap());

// Matches a lowercase letter or digit, optionally preceded by non-word characters
// (e.g. a space or punctuation). Used by languages that extend the base continuation
// check with their own month lists.
static CONTINUE_AFTER_NONWORD_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\W*[0-9a-z]").unwrap());

// Continuation rule for ellipsis matches. Treats the run as mid-sentence when
// the following text starts with whitespace and then a lowercase/digit
// (`... no`, `. . . what`), or with whitespace and the standalone pronoun `I`
// (`. . . I didn't`, `... I'm`). `I` is always capitalized in English and
// cannot be distinguished from a sentence start by case alone, so it is
// treated as continuation; other capitals (`No`, `Then`, `And`) still mark a
// boundary.
static ELLIPSIS_CONTINUE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+(?:[0-9a-z]|I(?:[\s'\u{2019}]|$))").unwrap()
});

// Glued lowercase directly after an ellipsis (`mean...see`) is intra-utterance
// hesitation and continues the sentence. Only fires when the dots are also
// glued behind (preceding char is non-whitespace); a free-standing leading
// ellipsis (`raak. ...en`) keeps its boundary.
static ELLIPSIS_GLUED_CONTINUE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-z]").unwrap());

static PARA_SPLIT_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n[\r]*\n").unwrap());

/// Push `boundary` only if it advances past the last recorded position.
/// Quote-extension can move a boundary past later regex matches in the same
/// paragraph; this keeps the boundary list strictly increasing.
fn push_if_increasing(boundaries: &mut Vec<usize>, boundary: usize) {
    if boundary > *boundaries.last().unwrap() {
        boundaries.push(boundary);
    }
}

/// Find all terminator-run matches in `text`, then fold a stray spaced `.`
/// onto any preceding `!`/`?`/`…` run so inputs like `Bravo ! .` don't
/// surface an orphan one-char sentence. The regex already coalesces
/// homogeneous runs (`! !`, `? ? ?`, `. . .`); the mixed case can't be
/// expressed without lookahead because `[!?…][ \t]+\.` would also eat the
/// first dot of an ellipsis (`! ...`). Restricting the merge to a single
/// trailing dot sidesteps that.
fn find_terminator_matches(text: &str, regex: &Regex) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out: Vec<(usize, usize)> = Vec::new();
    
    for m in regex.find_iter(text) {
        let (start, end) = (m.start(), m.end());

        if let Some(last) = out.last_mut() {
            // Order: cheap & rare-true predicates first. `last_emphatic`
            // is only true when the previous match ended in `!`/`?`/`…`,
            // which is uncommon - short-circuit on it before walking the
            // gap or doing slice work.
            let last_emphatic = text[last.0..last.1].ends_with(['!', '?', '…']);
            
            if last_emphatic && end - start == 1 && bytes[start] == b'.' {
                let gap = &bytes[last.1..start];

                if !gap.is_empty() && gap.iter().all(|&b| b == b' ' || b == b'\t') {
                    last.1 = end;
                    continue;
                }
            }
        }

        out.push((start, end));
    }

    out
}

/// Shared helper for languages that continue sentences before month names.
///
/// Returns `true` if `text` starts with a lowercase letter/digit (after optional
/// non-word characters), or if its first whitespace-delimited word (case-insensitively
/// capitalised) is one of the supplied `months`.
pub fn continues_after_boundary(text: &str, months: &[&str]) -> bool {
    if CONTINUE_AFTER_NONWORD_REGEX.is_match(text) {
        return true;
    }

    let next_word = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(['.', '!', '?']);

    if next_word.is_empty() {
        return false;
    }

    // Build a version with the first character upper-cased (handles non-ASCII safely).
    let capitalized: String = next_word
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i == 0 {
                c.to_uppercase().to_string()
            } else {
                c.to_string()
            }
        })
        .collect();

    months.contains(&next_word) || months.contains(&capitalized.as_str())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SkippableRangeType {
    Quote,
    Parentheses,
    Email,
    ListItem,
}

#[derive(Debug, Clone, Copy)]
pub struct SkippableRange {
    pub start: usize,
    pub end: usize,
    pub range_type: SkippableRangeType,
}

impl SkippableRange {
    pub fn new(start: usize, end: usize, range_type: SkippableRangeType) -> Self {
        Self {
            start,
            end,
            range_type,
        }
    }

    pub fn contains(&self, position: usize) -> bool {
        position > self.start && position < self.end
    }

    pub fn is_quote(&self) -> bool {
        self.range_type == SkippableRangeType::Quote
    }

    pub fn is_inner_terminator(&self, text: &str, boundary: usize) -> bool {
        if !self.is_quote() || boundary >= self.end {
            return false;
        }

        let head = &text[..self.end];
        QUOTE_CLOSERS_BY_LEN
            .iter()
            .any(|c| head.ends_with(*c) && boundary + c.len() == self.end)
    }
}

pub trait Language {
    /// Returns a reference to the compiled regex pattern that matches sentence terminating
    /// punctuation. The default implementation uses a static LazyLock for zero-cost access.
    fn get_sentence_break_regex(&self) -> &'static Regex {
        &DEFAULT_SENTENCE_BREAK_REGEX
    }

    /// Analyzes the input text and returns a vector of sentence boundaries.
    /// This is the main method for sentence segmentation that:
    /// 1. Splits text into paragraphs at double newlines
    /// 2. Identifies potential sentence breaks using regex patterns
    /// 3. Filters out false positives (abbreviations, quotes, etc.)
    /// 4. Returns structured boundary information including start/end positions and boundary symbols
    /// Each boundary contains the sentence text, position indices, and metadata about the boundary type.
    fn get_sentence_boundaries<'a>(&self, text: &'a str) -> Vec<SentenceBoundary<'a>> {
        // Pre-allocate boundaries with estimated capacity (rough estimate: 1 sentence per 50 characters)
        let estimated_sentences = (text.len() / 50).max(1);
        let mut boundaries = Vec::with_capacity(estimated_sentences);

        // Split by paragraph breaks (one or more newlines with optional whitespace)
        let paragraphs: Vec<&str> = PARA_SPLIT_REGEX.split(text).collect();

        // Pre-calculate all paragraph offsets in one pass
        // CRITICAL: We track both byte offsets AND character offsets separately.
        // This is essential for correct handling of multi-byte UTF-8 characters (e.g., CJK, emoji).
        //
        // - `paragraph_offsets`: byte indices into the original text (for slicing with &text[start..end])
        // - `paragraph_char_offsets`: character counts (for SentenceBoundary.start_index/end_index)
        //
        // Example: "日本語" is 3 characters but 9 bytes in UTF-8:
        //   - byte offset: 0..9
        //   - char offset: 0..3
        let mut paragraph_offsets = Vec::with_capacity(paragraphs.len());
        let mut current_offset = 0;
        let mut paragraph_char_offsets = Vec::with_capacity(paragraphs.len());
        let mut current_char_offset = 0;
        for (i, paragraph) in paragraphs.iter().enumerate() {
            paragraph_offsets.push(current_offset);
            paragraph_char_offsets.push(current_char_offset);
            current_offset += paragraph.len();
            current_char_offset += paragraph.chars().count();
            if i < paragraphs.len() - 1 {
                current_offset += 2; // for "\n\n" bytes
                current_char_offset += 2; // for "\n\n" chars (always 2, regardless of encoding)
            }
        }

        // Pre-allocate sentence_boundaries once and reuse for all paragraphs
        let estimated_paragraph_sentences = 10; // reasonable default for typical paragraphs
        let mut sentence_boundaries = Vec::with_capacity(estimated_paragraph_sentences);

        for (pindex, paragraph) in paragraphs.iter().enumerate() {
            if pindex > 0 {
                let paragraph_start = paragraph_offsets[pindex];
                let paragraph_char_start = paragraph_char_offsets[pindex];
                boundaries.push(SentenceBoundary {
                    start_index: paragraph_char_start - 2,
                    end_index: paragraph_char_start,
                    start_byte: paragraph_start - 2,
                    end_byte: paragraph_start,
                    text: "\n\n",
                    boundary_symbol: None,
                    is_paragraph_break: true,
                });
            }

            let paragraph_start_offset = if pindex == 0 {
                0
            } else {
                paragraph_offsets[pindex]
            };

            let paragraph_start_char_offset = if pindex == 0 {
                0
            } else {
                paragraph_char_offsets[pindex]
            };

            sentence_boundaries.clear();
            sentence_boundaries.push(0);

            let matches = find_terminator_matches(paragraph, self.get_sentence_break_regex());
            let mut skippable_ranges = self.get_skippable_ranges(paragraph);

            // Detect list-item line starts once per paragraph and reuse the
            // result for both atomic-item ranges (so terminator-driven boundaries
            // inside an item are dropped) and explicit boundary emission below.
            let list_starts = self.list_items(paragraph).unwrap_or_default();
            if !list_starts.is_empty() {
                for window in list_starts.windows(2) {
                    skippable_ranges.push(SkippableRange::new(
                        window[0],
                        window[1],
                        SkippableRangeType::ListItem,
                    ));
                }
                let last = *list_starts.last().unwrap();
                skippable_ranges.push(SkippableRange::new(
                    last,
                    paragraph.len(),
                    SkippableRangeType::ListItem,
                ));
                skippable_ranges.sort_unstable_by_key(|r| r.start);
            }

            'next_match: for (start, end) in matches {
                let Some(mut boundary) = self.find_boundary(paragraph, start, end) else {
                    continue;
                };

                for range in &skippable_ranges {
                    if !range.contains(boundary) {
                        continue;
                    }

                    // Inside a quoted/parens/email range. Either advance past the
                    // closer (if the boundary sits at an inner terminator) or drop
                    // this match entirely. Either way, no further extension applies.
                    if range.is_inner_terminator(paragraph, boundary) {
                        let next_word = self.get_next_word_approx(paragraph, range.end);
                        let extend = self.get_boundary_extend(next_word);
                        if extend >= 0 {
                            push_if_increasing(
                                &mut sentence_boundaries,
                                range.end + extend as usize,
                            );
                        }
                    }
                    continue 'next_match;
                }

                boundary = self.extend_past_orphan_closer(paragraph, boundary, &skippable_ranges);
                push_if_increasing(&mut sentence_boundaries, boundary);
            }

            // Merge in list-item line starts as sentence boundaries. They may
            // interleave with terminator boundaries in source order, so we
            // sort + dedup once rather than maintain the increasing invariant
            // during insertion.
            if !list_starts.is_empty() {
                for &start in &list_starts {
                    if start > 0 {
                        sentence_boundaries.push(start);
                    }
                }
                sentence_boundaries.sort_unstable();
                sentence_boundaries.dedup();
            }

            if *sentence_boundaries.last().unwrap() != paragraph.len() {
                sentence_boundaries.push(paragraph.len());
            }

            let mut prev_end_index = paragraph_start_char_offset;
            let mut prev_end_byte = 0;

            for i in 0..sentence_boundaries.len() - 1 {
                let start = sentence_boundaries[i];
                let end = sentence_boundaries[i + 1];

                if start >= paragraph.len() || end > paragraph.len() || start > end {
                    continue;
                }

                let sentence_text = &paragraph[start..end];
                let boundary_symbol = if end > 0 && end <= paragraph.len() {
                    // Trim trailing whitespace before looking for the boundary symbol.
                    // This fixes the issue where boundary symbols are not detected when
                    // followed by whitespace (e.g., "Hello. " should detect "." as symbol).
                    let sentence_slice = &paragraph[..end];
                    let trimmed_slice = sentence_slice.trim_end();

                    // Use char_indices for more efficient character iteration on the trimmed slice
                    trimmed_slice
                        .char_indices()
                        .next_back()
                        .and_then(|(idx, _)| {
                            // Extract the last character from the trimmed slice
                            let char_str = &trimmed_slice[idx..];
                            if GLOBAL_SENTENCE_TERMINATORS.contains(&char_str) {
                                Some(char_str.to_string())
                            } else {
                                None
                            }
                        })
                } else {
                    None
                };

                let start_byte = paragraph_start_offset + start;
                let end_byte = paragraph_start_offset + end;

                let start_index = if start == prev_end_byte {
                    prev_end_index
                } else {
                    let safe_prev = paragraph.floor_char_boundary(prev_end_byte);
                    let safe_start = paragraph.floor_char_boundary(start);
                    prev_end_index + paragraph[safe_prev..safe_start].chars().count()
                };
                let end_index = start_index + sentence_text.chars().count();

                boundaries.push(SentenceBoundary {
                    start_index,
                    end_index,
                    start_byte,
                    end_byte,
                    text: sentence_text,
                    boundary_symbol,
                    is_paragraph_break: false,
                });

                prev_end_index = end_index;
                prev_end_byte = end;
            }
        }

        boundaries
    }

    /// Segments the input text into individual sentences and returns them as string slices.
    /// This is a convenience method that builds on get_sentence_boundaries() but returns
    /// only the sentence text content without the additional boundary metadata.
    /// Used when you only need the segmented sentences and not their position information.
    fn segment<'a>(&self, text: &'a str) -> Vec<&'a str> {
        // Pre-allocate with estimated capacity based on text length
        let estimated_sentences = (text.len() / 50).max(1);
        let mut sentences = Vec::with_capacity(estimated_sentences);

        let boundaries = self.get_sentence_boundaries(text);
        for boundary in boundaries {
            if !boundary.text.is_empty() {
                sentences.push(boundary.text);
            }
        }

        sentences
    }

    /// Returns the character used to mark abbreviations in this language.
    /// By default returns "." (period), but should be overridden by specific languages
    /// that use different abbreviation markers. Used by the abbreviation detection logic
    /// to determine if a potential sentence boundary is actually an abbreviation.
    fn get_abbreviation_char(&self) -> &str {
        "."
    }

    /// Returns a list of known abbreviations for this language.
    /// These are used to prevent false sentence breaks at abbreviation periods.
    /// For example, "Dr." or "etc." should not trigger a sentence boundary.
    /// Languages should override this to provide their specific abbreviation lists.
    /// Returns an empty slice by default.
    fn get_abbreviations(&self) -> &[String] {
        &[]
    }

    /// Determines how many characters to extend a boundary when continuing into the next word.
    /// Returns -1 if the word indicates the boundary should not be created (continuation case).
    /// Returns 0 or positive number indicating how many whitespace/punctuation characters
    /// to skip when positioning the boundary. Used to handle cases like quoted sentences
    /// where the boundary should include trailing punctuation and whitespace.
    fn get_boundary_extend(&self, word: &str) -> i8 {
        if self.continue_in_next_word(word.trim()) || CONTINUE_AFTER_NONWORD_REGEX.is_match(word) {
            // not a boundary.
            return -1;
        }

        let mut count = 0i8;
        for ch in word.chars() {
            if ch.is_whitespace() || GLOBAL_SENTENCE_TERMINATORS.contains(&ch.to_string().as_str())
            {
                count += 1;
                if count == i8::MAX {
                    break; // Prevent overflow
                }
            } else {
                break;
            }
        }

        word.ceil_char_boundary(count as usize) as i8
    }

    /// If `boundary` sits at an orphan trailing quote closer (e.g. `.'` with no
    /// matching opener captured by `QUOTES_REGEX`), advance past the closer, any
    /// trailing whitespace, and any stranded terminator that would otherwise
    /// form a single-punctuation sentence. Returns `boundary` unchanged otherwise.
    fn extend_past_orphan_closer(
        &self,
        paragraph: &str,
        boundary: usize,
        skippable_ranges: &[SkippableRange],
    ) -> usize {
        // If the next char opens a known quoted range, that quote belongs to
        // the upcoming sentence — leave the boundary alone.
        if skippable_ranges.iter().any(|r| r.start == boundary) {
            return boundary;
        }

        // For ambiguous pairs (opener == closer, e.g. `'` or `"`) the regex
        // may fail to pair both ends when they are space-padded. Use parity
        // of the closer in the preceding text: odd ⇒ unmatched opener exists
        // ⇒ true orphan (pull it); even ⇒ already paired ⇒ opening a new
        // phrase (leave it).
        let is_orphan = |closer: &str| -> bool {
            let symmetric = QUOTE_PAIRS
                .iter()
                .any(|p| p.open == p.close && p.close == closer);
            !symmetric || paragraph[..boundary].matches(closer).count() % 2 == 1
        };

        let Some(closer) = QUOTE_CLOSERS_BY_LEN
            .iter()
            .find(|c| paragraph[boundary..].starts_with(**c) && is_orphan(c))
        else {
            return boundary;
        };

        let advance_past_space = |pos: usize| {
            SPACE_AFTER_SEPARATOR
                .find(&paragraph[pos..])
                .map_or(pos, |m| pos + m.end())
        };

        let mut boundary = advance_past_space(boundary + closer.len());
        // Absorb any stranded terminators (e.g. `'' .`) that would otherwise
        // form a single-punctuation sentence.
        while let Some(m) = self
            .get_sentence_break_regex()
            .find(&paragraph[boundary..])
            .filter(|m| m.start() == 0)
        {
            boundary = advance_past_space(boundary + m.end());
        }

        boundary
    }

    /// Checks if a potential sentence boundary is actually part of an abbreviation.
    /// Examines the text before the separator to see if it ends with a known abbreviation.
    /// Returns true if this appears to be an abbreviation (and thus not a sentence boundary),
    /// false if it's likely a genuine sentence end. Used to prevent breaking sentences
    /// at abbreviations like "Dr. Smith" or "etc."
    fn is_abbreviation(&self, head: &str, _tail: &str, separator: &str) -> bool {
        if self.get_abbreviation_char() != separator {
            return false;
        }

        let last_word = self.get_last_word(head);

        if last_word.is_empty() {
            return false;
        }

        let abbreviations = self.get_abbreviations();
        let is_abbrev = abbreviations.contains(&last_word.to_string());
        let is_abbrev_lower = abbreviations.contains(&last_word.to_lowercase());
        let is_abbrev_upper = abbreviations.contains(&last_word.to_uppercase());

        is_abbrev || is_abbrev_lower || is_abbrev_upper
    }

    /// Extracts the last word from the given text by splitting on whitespace and periods.
    /// Used primarily by abbreviation detection to check if the word before a potential
    /// sentence boundary is a known abbreviation. Returns an empty string if no words
    /// are found. This is a performance-optimized version that avoids collecting all words.
    fn get_last_word<'a>(&self, text: &'a str) -> &'a str {
        // Trim trailing whitespace so a stray space before the terminator
        // (`U.S .`) doesn't blank out the last word. `/` joins route names
        // to abbreviations (`171/U.S`) without being a real word boundary,
        // so split on it too. Walk back from the end (rfind) rather than
        // splitting from the start: this is on the per-match hot path and
        // we only need the trailing word.
        let trimmed = text.trim_end();
        match trimmed
            .char_indices()
            .rfind(|(_, c)| c.is_whitespace() || *c == '.' || *c == '/')
        {
            Some((i, c)) => &trimmed[i + c.len_utf8()..],
            None => trimmed,
        }
    }

    /// Checks if a potential sentence boundary is actually an exclamation word that shouldn't
    /// trigger a sentence break. Examines the last word before the boundary and checks if
    /// it's in the list of known exclamation words (like "Hey!" or "Wow!").
    /// Returns true if this is an exclamation that should not break the sentence.
    fn is_exclamation(&self, head: &str, _tail: &str) -> bool {
        let last_word = self.get_last_word(head);
        let exclamation_word = format!("{}!", last_word);
        EXCLAMATION_WORDS.contains(&exclamation_word.as_str())
    }

    /// Returns an approximate substring of the next word(s) starting from the given position.
    /// Limited to a maximum of 30 characters for performance. Used to analyze context
    /// after a potential sentence boundary to determine if the boundary should be created.
    /// Handles UTF-8 character boundaries safely to avoid panics on non-ASCII text.
    fn get_next_word_approx<'a>(&self, text: &'a str, start: usize) -> &'a str {
        if start >= text.len() {
            return "";
        }

        let max_chars = 30;
        let safe_start = text.floor_char_boundary(start);
        let end_pos = (start + max_chars).min(text.len());
        &text[safe_start..text.ceil_char_boundary(end_pos)]
    }

    /// Analyzes a potential sentence boundary and determines the exact position where
    /// the sentence should end, or returns None if this shouldn't be a boundary.
    /// Considers abbreviations, exclamations, numbered references, and continuation patterns.
    /// This is the core logic that distinguishes true sentence boundaries from false positives
    /// like abbreviations or mid-sentence punctuation.
    fn find_boundary(&self, text: &str, start: usize, end: usize) -> Option<usize> {
        let head = &text[..start];
        let matched = &text[start..end];

        // Any coalesced multi-char terminator run — adjacent (`...`, `!?`),
        // space-separated dots (`. . .`), or spaced emphatic mixes (`! ?`,
        // `! !`, `… ?`) — allows leading whitespace before the lowercase
        // continuation test, so `! ? is` and `... no` read as mid-sentence
        // rather than a boundary. `chars().nth(1)` distinguishes a real
        // multi-char run from a single multi-byte terminator (`。`, `…`).
        let is_multi_char_run = matched.chars().nth(1).is_some();

        // For any multi-char match (e.g. `...`, `!?`, `!...`), scan continuation
        // and trailing-space extension from the end of the run, not from one byte
        // past its first char.
        let next_index = if is_multi_char_run {
            end
        } else {
            text.ceil_char_boundary(start + 1)
        };

        let next_word_approx = self.get_next_word_approx(text, next_index);

        if let Some(number_ref_match) =
            crate::constants::NUMBERED_REFERENCE_REGEX.find(next_word_approx)
        {
            return Some(next_index + number_ref_match.end());
        }

        let continues = if is_multi_char_run {
            ELLIPSIS_CONTINUE_REGEX.is_match(next_word_approx)
                || (head
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace())
                    && ELLIPSIS_GLUED_CONTINUE_REGEX.is_match(next_word_approx))
        } else {
            self.continue_in_next_word(next_word_approx)
        };

        if continues {
            return None;
        }

        // Digit immediately before the period and a digit-bearing alphanumeric
        // token immediately after (no space) is a code-like numbered token,
        // not a sentence end: chess moves (`7.Bg5`). Requiring a digit in the
        // follower keeps quantities like `1,000.That` on the normal boundary
        // path. The lowercase variant (`7.f4`) already passes through
        // `continues`; this handles the uppercase case.
        if matched == "."
            && head.bytes().next_back().is_some_and(|b| b.is_ascii_digit())
            && next_word_approx
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic())
            && next_word_approx.bytes().any(|b| b.is_ascii_digit())
        {
            return None;
        }

        if self.is_abbreviation(head, next_word_approx, &text[start..end]) {
            return None;
        }

        if self.is_exclamation(head, next_word_approx) {
            return None;
        }

        if let Some(space_after_sep_match) =
            crate::constants::SPACE_AFTER_SEPARATOR.find(next_word_approx)
        {
            return Some(next_index + space_after_sep_match.end());
        }

        Some(end)
    }

    /// Determines if the text after a potential boundary indicates the sentence should continue.
    /// Returns true if the next word starts with a lowercase letter or number, suggesting
    /// the sentence is continuing rather than starting a new one. This helps avoid breaking
    /// sentences at abbreviations or in the middle of compound sentences.
    fn continue_in_next_word(&self, text_after_boundary: &str) -> bool {
        if CONTINUE_REGEX.is_match(text_after_boundary) {
            return true;
        }
        // A comma following the terminator (after optional spaces) signals that
        // the period is stray punctuation and the real clause continues. Treat
        // it as a continuation rather than a boundary.
        let trimmed = text_after_boundary.trim_start();
        trimmed.as_bytes().first() == Some(&b',')
    }

    /// Identifies ranges of text that should be skipped during sentence boundary detection.
    /// This includes quoted text, parenthetical expressions, and email addresses where
    /// internal punctuation should not trigger sentence breaks. Returns a sorted vector
    /// of ranges that can be efficiently checked during boundary detection to avoid
    /// false positives within these special text regions.
    fn get_skippable_ranges(&self, text: &str) -> Vec<SkippableRange> {
        // Pre-allocate with estimated capacity based on text length (rough estimate: 1 range per 200 characters)
        let estimated_ranges = (text.len() / 200).max(1);
        let mut skippable_ranges = Vec::with_capacity(estimated_ranges);

        for mat in QUOTES_REGEX.find_iter(text) {
            skippable_ranges.push(SkippableRange::new(
                mat.start(),
                mat.end(),
                SkippableRangeType::Quote,
            ));
        }

        for mat in PARENS_REGEX.find_iter(text) {
            skippable_ranges.push(SkippableRange::new(
                mat.start(),
                mat.end(),
                SkippableRangeType::Parentheses,
            ));
        }

        for mat in EMAIL_REGEX.find_iter(text) {
            skippable_ranges.push(SkippableRange::new(
                mat.start(),
                mat.end(),
                SkippableRangeType::Email,
            ));
        }

        // Sort ranges by start position for more efficient lookups
        skippable_ranges.sort_unstable_by_key(|r| r.start);
        skippable_ranges
    }

    /// Returns byte offsets of list-item line starts within `paragraph`.
    /// Each returned offset becomes a sentence boundary, and the span between
    /// consecutive offsets is treated as a single atomic item (terminator-driven
    /// breaks inside an item span are dropped).
    ///
    /// Default impl uses the language-agnostic `detect_list_items` scanner, which
    /// handles ASCII bullets, Unicode bullets, numeric (`1.`/`1)`), parenthesised
    /// (`(1)`/`(a)`/`(ii)`), letter+paren (`a)`), lowercase letter+dot (`a.`),
    /// and multi-char roman (`ii.`/`iii)`) markers. Override to add language-specific
    /// markers (e.g. CJK enumerators) or return `None` to opt out entirely.
    fn list_items(&self, paragraph: &str) -> Option<Vec<usize>> {
        Some(super::list_markers::detect_list_items(paragraph))
    }
}
