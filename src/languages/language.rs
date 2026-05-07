use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

use crate::SentenceBoundary;
use crate::constants::EMAIL_REGEX;
use crate::constants::EXCLAMATION_WORDS;
use crate::constants::GLOBAL_SENTENCE_TERMINATORS;
use crate::constants::GLOBAL_SENTENCE_TERMINATORS_SET;
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

// Inline sentence-break signature inside a candidate `'…'` quote span: a `.`
// or `!` followed by `[ \t]+` and an ASCII uppercase letter. `?` is excluded
// because quoted utterances ending in `?` commonly continue the surrounding
// sentence (`… boy? ' he said, …`); that case is handled separately by
// `continuation_after_orphan_quote`.
static INLINE_SENTENCE_BREAK_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[.!][ \t]+[A-Z]").unwrap());

/// True when `closer` is the closing token of a symmetric quote pair —
/// `'`, `''`, `"`, etc., where opener equals closer. These pairs are
/// inherently ambiguous: `QUOTES_REGEX` can fail to pair them when they're
/// space-padded, so callers need extra logic to decide orphanhood.
fn is_symmetric_quote_closer(closer: &str) -> bool {
    QUOTE_PAIRS
        .iter()
        .any(|p| p.open == p.close && p.close == closer)
}

/// True when the `closer` token at `boundary` in `paragraph` is an orphan —
/// either it has no matching opener earlier in the paragraph, or it is a
/// lone stray with no counterpart anywhere.
///
/// Asymmetric closers (`»`, `”`, …) are unambiguous and always orphan once
/// `QUOTES_REGEX` has had a chance to pair them. For symmetric closers we
/// inspect the closer's distribution across the paragraph, ignoring any
/// occurrence already consumed by an asymmetric quote pair.
fn is_orphan_closer(
    paragraph: &str,
    boundary: usize,
    closer: &str,
    skippable_ranges: &[SkippableRange],
) -> bool {
    if !is_symmetric_quote_closer(closer) {
        return true;
    }

    let (mut before, mut at_or_after) = (0usize, 0usize);
    for (idx, _) in paragraph.match_indices(closer) {
        if is_in_quote_range(skippable_ranges, idx) || is_contraction_quote(paragraph, idx, closer)
        {
            continue;
        }

        if idx < boundary {
            before += 1;
        } else {
            at_or_after += 1;
        }
    }

    let unmatched_opener_before = before % 2 == 1;
    let lone_stray_at_boundary = before == 0 && at_or_after == 1;
    unmatched_opener_before || lone_stray_at_boundary
}

/// True when `range` is a quote range whose opener and closer are the same
/// token — `''…''`, `'…'`, `"…"`, etc. These pairs are inherently ambiguous
/// for the regex pairer and need extra scrutiny when they appear to veto a
/// sentence boundary.
fn is_symmetric_quote_range(text: &str, range: &SkippableRange) -> bool {
    if !range.is_quote() {
        return false;
    }
    let span = &text[range.start..range.end];
    QUOTE_PAIRS
        .iter()
        .filter(|p| p.open == p.close)
        .any(|p| span.len() >= 2 * p.open.len() && span.starts_with(p.open) && span.ends_with(p.close))
}

/// True when the paragraph contains an odd number of the symmetric quote
/// token that opens `range`. An odd count guarantees at least one orphan
/// occurrence — and when that orphan sits earlier than a real downstream
/// opener, `QUOTES_REGEX` will mispair across a real sentence break. Even
/// counts are structurally consistent and should be trusted.
fn symmetric_token_count_is_odd(paragraph: &str, range: &SkippableRange) -> bool {
    let Some(pair) = QUOTE_PAIRS
        .iter()
        .filter(|p| p.open == p.close)
        .find(|p| paragraph[range.start..].starts_with(p.open))
    else {
        return false;
    };
    paragraph.matches(pair.open).count() % 2 == 1
}

/// True when `quote` and any parens range in `ranges` partially overlap —
/// one endpoint inside, the other outside. Full containment in either
/// direction (a quote wrapping parens, or parens wrapping a quote) is fine
/// and returns false. Partial overlap is the signature of a greedy
/// symmetric-pair pairing that crossed what's really a sentence break.
fn quote_partially_overlaps_parens(quote: &SkippableRange, ranges: &[SkippableRange]) -> bool {
    ranges
        .iter()
        .filter(|r| r.range_type == SkippableRangeType::Parentheses)
        .any(|p| {
            (quote.start < p.start && p.start < quote.end && quote.end < p.end)
                || (p.start < quote.start && quote.start < p.end && p.end < quote.end)
        })
}

/// True when `idx` lies inside an existing quote `SkippableRange`.
fn is_in_quote_range(ranges: &[SkippableRange], idx: usize) -> bool {
    ranges
        .iter()
        .any(|r| r.is_quote() && idx >= r.start && idx < r.end)
}

/// Append `'…'` / `` `…` `` ranges that `QUOTES_REGEX` couldn't pair. The
/// guarded patterns require a `\b` immediately after the opener, so
/// space-padded openers like `' word ` go unpaired even when they form a
/// real `' … '` pair — the shape that `''(?s:.*?)''` matches natively for
/// unguarded `''`. Without this, replacing `''` with `'` in identical text
/// changes segmentation; with it, the two behave the same.
///
/// Algorithm: for each guarded symmetric token, collect candidate positions
/// (skipping contractions and tokens already inside a quote range), then
/// walk consecutive pairs. Pair an opener-shape position with its immediate
/// neighbour unless the candidate closer is itself opener-like — followed
/// by whitespace + uppercase, suggesting it is opening another utterance
/// rather than closing this one — AND the intervening span contains an
/// inline `[.!] + ws + uppercase` sentence break. That combined signal
/// distinguishes back-to-back utterances (`' utt1. ' Utt2 '`, where the
/// middle `'` is a fresh opener) from a single multi-sentence quotation
/// (`' s1. s2. ' he said`, where the closing `'` is followed by narrative).
fn append_space_padded_quote_pairs(text: &str, ranges: &mut Vec<SkippableRange>) {
    for pair in QUOTE_PAIRS.iter().filter(|p| p.guard && p.open == p.close) {
        let token = pair.close;
        let positions: Vec<usize> = text
            .match_indices(token)
            .map(|(idx, _)| idx)
            .filter(|&idx| {
                !is_in_quote_range(ranges, idx) && !is_contraction_quote(text, idx, token)
            })
            .collect();

        let mut candidates = positions.iter().copied().peekable();
        while let Some(opener) = candidates.next() {
            let Some(&closer) = candidates.peek() else { break };
            let span = &text[opener + token.len()..closer];
            if is_opener_shape(text, opener, token)
                && !(starts_new_utterance(text, closer + token.len())
                    && INLINE_SENTENCE_BREAK_REGEX.is_match(span))
            {
                ranges.push(SkippableRange::new(
                    opener,
                    closer + token.len(),
                    SkippableRangeType::Quote,
                ));
                candidates.next();
            }
        }
    }
}

/// True when `text[from..]` starts with whitespace followed by an ASCII
/// uppercase letter — the signature of a fresh utterance/sentence opening.
fn starts_new_utterance(text: &str, from: usize) -> bool {
    text[from..]
        .trim_start_matches(|c: char| c == ' ' || c == '\t')
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
        && text[from..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
}

/// True when the `token` occurrence at `idx` is a contraction (`wasn't`,
/// `o'clock`) — sandwiched between two alphanumerics. Such `'` are not
/// quote characters; counting them flips the parity-based orphan check.
fn is_contraction_quote(text: &str, idx: usize, token: &str) -> bool {
    let prev_word = text[..idx]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric());

    let next_word = text[idx + token.len()..]
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric());
    prev_word && next_word
}

/// True when the `'` (or `` ` ``) at `idx` is preceded by start-of-text or
/// whitespace AND followed by whitespace — the shape `QUOTES_REGEX`'s
/// guarded pattern can't pair (it requires `\b` after the opener). The
/// fallback pairer in `get_skippable_ranges` keys on this shape.
fn is_opener_shape(text: &str, idx: usize, token: &str) -> bool {
    let prev = text[..idx].chars().next_back();
    let next = text[idx + token.len()..].chars().next();
    (prev.is_none() || prev.is_some_and(char::is_whitespace))
        && next.is_some_and(char::is_whitespace)
}

/// Push `boundary` only if it advances past the last recorded position.
/// Quote-extension can move a boundary past later regex matches in the same
/// paragraph; this keeps the boundary list strictly increasing.
fn push_if_increasing(boundaries: &mut Vec<usize>, boundary: usize) {
    if boundary > *boundaries.last().unwrap() {
        boundaries.push(boundary);
    }
}

/// True iff `s` is a single ASCII uppercase letter. Used to recognise name
/// initials and gate the structural / starter override.
fn is_single_ascii_upper(s: &str) -> bool {
    s.len() == 1 && s.as_bytes()[0].is_ascii_uppercase()
}

/// True iff `s` (after leading whitespace) begins with a name-initial token:
/// a single uppercase ASCII letter, a `.`, then end-of-string or whitespace.
/// `J. R. Tolkien` triggers; `Jones`, `J.R.R.`, and `A.B` do not.
fn starts_with_initial(s: &str) -> bool {
    let mut chars = s.trim_start().chars();
    let Some(first) = chars.next() else { return false };
    first.is_ascii_uppercase()
        && chars.next() == Some('.')
        && chars.next().is_none_or(char::is_whitespace)
}

/// True when the time phrase ending at `head` is "fronted" - the preposition
/// introducing the time literal starts with an uppercase letter, so the time
/// phrase opens a sentence (`At 5 a.m.`, `By 9:00 P.M.`) rather than closing
/// one (`he left at 6 p.m.`). Caller must already have established that
/// `head` ends in a 3-char time-abbrev suffix (`a.m`/`p.m`, any case).
fn time_phrase_is_fronted(head: &str) -> bool {
    let after_suffix = head[..head.len() - 3].trim_end();
    let after_num = after_suffix
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == ':')
        .trim_end();
    after_num
        .rsplit(char::is_whitespace)
        .next()
        .and_then(|w| w.chars().next())
        .is_some_and(|c| c.is_ascii_uppercase())
}

/// True when `text` — the slice immediately after an orphan `!`/`?` — looks
/// like a mid-sentence continuation rather than a new sentence.
///
/// Three named steps:
///   1. trim leading whitespace,
///   2. optionally peel off one symmetric-quote token (`'`, `''`, `"`, `` ` ``,
///      ` `` `, `‚`, `‛`, `‟`) and the whitespace that follows — covers cases
///      like `! '' and …` where an odd count of `''` leaves a stray closer
///      between the terminator and the real continuation,
///   3. peek the next char: lowercase-ASCII or digit means continuation.
///
/// Step 2 reuses [`QUOTE_PAIRS`] (filtered to symmetric pairs where opener and
/// closer are the same token) so the set of quote tokens stays in sync with
/// the rest of the segmenter — no hand-maintained ASCII list.
fn continuation_after_orphan_quote(text: &str) -> bool {
    let after_ws = text.trim_start();
    let after_quote = QUOTE_PAIRS
        .iter()
        .filter(|p| p.open == p.close)
        .find_map(|p| after_ws.strip_prefix(p.close))
        .map(str::trim_start)
        .unwrap_or(after_ws);

    after_quote
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
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

                    // Symmetric-pair quote ranges (`''…''`, `'…'`, `"…"`) are
                    // greedy: when a paragraph has more than one closer the
                    // regex can pair across what's really a sentence break.
                    // Detect that mispairing structurally — the range's
                    // endpoints straddle a parens boundary — and let the
                    // boundary through instead of vetoing it.
                    if is_symmetric_quote_range(paragraph, range)
                        && (quote_partially_overlaps_parens(range, &skippable_ranges)
                            || (symmetric_token_count_is_odd(paragraph, range)
                                && self.has_strong_sentence_break(paragraph, start, end)))
                    {
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
                        .and_then(|(idx, ch)| {
                            if GLOBAL_SENTENCE_TERMINATORS_SET.contains(&ch) {
                                Some(trimmed_slice[idx..].to_string())
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
    fn get_abbreviations(&self) -> &HashSet<String> {
        static EMPTY_ABBREVS: LazyLock<HashSet<String>> = LazyLock::new(HashSet::new);
        &EMPTY_ABBREVS
    }

    /// Returns a list of common sentence-opener words for this language.
    /// Used as a one-way override: when the abbreviation or name-initial path
    /// would suppress a boundary but the next word is a known sentence opener
    /// (e.g. "The", "He", "Did"), the boundary is forced back on.
    /// Restrict the list to function words and auxiliaries that almost never
    /// appear capitalized mid-sentence; never include proper nouns. Returns an
    /// empty set by default - languages opt in by overriding.
    fn get_sentence_starters(&self) -> &HashSet<String> {
        static EMPTY_STARTERS: LazyLock<HashSet<String>> = LazyLock::new(HashSet::new);
        &EMPTY_STARTERS
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
            if ch.is_whitespace() || GLOBAL_SENTENCE_TERMINATORS_SET.contains(&ch) {
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

        // Find an orphan closer starting at `boundary`, if any. Longest first
        // so `''` wins over `'` when both could match.
        let Some(closer) = QUOTE_CLOSERS_BY_LEN.iter().find(|c| {
            paragraph[boundary..].starts_with(**c)
                && is_orphan_closer(paragraph, boundary, c, skippable_ranges)
        }) else {
            return boundary;
        };

        // A symmetric closer (`''`, `'`, `"`, …) that is whitespace-separated
        // from the terminator and followed by a capitalized word is far more
        // likely the *opener* of the next sentence than a trailing orphan of
        // the current one. Only apply this when there are no unconsumed
        // occurrences of the same token before `boundary` — i.e. we hit the
        // `before == 0 && at_or_after == 1` branch in `is_orphan_closer`. The
        // odd-unmatched-opener case (`before > 0`) is a real trailing closer
        // and must keep its current behaviour.
        // A symmetric closer (`''`, `'`, `"`, …) that follows a terminator+space
        // and precedes whitespace + a capitalized word is more plausibly the
        // *opener* of the next sentence than a trailing orphan — but only when
        // the same token has already been used as a paired opener/closer
        // earlier in the paragraph. That paired use is the signal that the
        // text is using this token in opener position too; without it, the
        // token is more likely a stray closer (e.g. `… do ? ''` with no
        // earlier opener).
        if is_symmetric_quote_closer(closer) {
            let has_earlier_symmetric_pair = skippable_ranges.iter().any(|r| {
                r.end <= boundary
                    && is_symmetric_quote_range(paragraph, r)
                    && paragraph[r.start..].starts_with(*closer)
            });
            if has_earlier_symmetric_pair {
                let after = &paragraph[boundary + closer.len()..];
                let mut chars = after.chars();
                let first = chars.next();
                if first.is_some_and(char::is_whitespace) {
                    let next_non_ws = chars.find(|c| !c.is_whitespace());
                    if next_non_ws.is_some_and(|c| c.is_ascii_uppercase()) {
                        return boundary;
                    }
                }
            }
        }

        // Pull the boundary past the closer, then mop up trailing whitespace
        // and any stranded terminators (`'' .`) that would otherwise form a
        // single-punctuation sentence on their own.
        let advance_past_space = |pos: usize| {
            SPACE_AFTER_SEPARATOR
                .find(&paragraph[pos..])
                .map_or(pos, |m| pos + m.end())
        };

        let mut boundary = advance_past_space(boundary + closer.len());
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
    fn is_abbreviation(&self, _head: &str, last_word: &str, separator: &str) -> bool {
        if self.get_abbreviation_char() != separator || last_word.is_empty() {
            return false;
        }

        let abbreviations = self.get_abbreviations();

        abbreviations.contains(last_word)
            || abbreviations.contains(last_word.to_lowercase().as_str())
            || abbreviations.contains(last_word.to_uppercase().as_str())
    }

    /// Detects a name initial: a single uppercase ASCII letter followed by a
    /// period in a position that looks like part of a name. Returns true when
    /// the immediately preceding token in `head` starts with an uppercase
    /// ASCII letter (`Albert I.`, `George W.`) or the immediately following
    /// token is itself an initial (`J. R. R. Tolkien`, including the
    /// sentence-initial position where there is no preceding token).
    ///
    /// Conservative: ASCII-only on both sides, so non-Latin scripts are
    /// unaffected. Caller is expected to gate this on the matched terminator
    /// being a single `.` and on `last_word` being a single uppercase letter;
    /// the helper re-checks the latter so it is safe to call standalone.
    fn is_name_initial(&self, head: &str, last_word: &str, next_word_approx: &str) -> bool {
        if !is_single_ascii_upper(last_word) {
            return false;
        }

        // Preceding-token rule: trim the initial and any separators
        // get_last_word splits on (whitespace, `.`, `/`), then take the
        // trailing word of what's left.
        let prefix = head[..head.len() - last_word.len()]
            .trim_end_matches(|c: char| c.is_whitespace() || c == '.' || c == '/');
            
        let prev_token = self.get_last_word(prefix);
        if prev_token
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
        {
            return true;
        }

        // Chained-initial rule: the next token is itself an initial.
        starts_with_initial(next_word_approx)
    }

    /// Returns true if the next non-space token in `next_word_approx` is a
    /// known sentence-opener for this language. Used as a one-way override
    /// when the abbreviation or name-initial path would otherwise suppress a
    /// boundary - a function-word opener (`The`, `He`, `Did`, ...) strongly
    /// signals the start of a new sentence.
    fn next_word_is_sentence_starter(&self, next_word_approx: &str) -> bool {
        let starters = self.get_sentence_starters();
        if starters.is_empty() {
            return false;
        }
        let trimmed = next_word_approx.trim_start();
        // Stop at whitespace, any sentence terminator (covers `.`, `!`, `?`,
        // `…`, and the ~150 Unicode terminators across scripts), or a comma -
        // the most likely trailing punctuation immediately after a starter
        // word (`I. Did, however, ...`).
        let word_end = trimmed
            .find(|c: char| {
                c.is_whitespace()
                    || c == ','
                    || crate::constants::GLOBAL_SENTENCE_TERMINATORS_SET.contains(&c)
            })
            .unwrap_or(trimmed.len());
        word_end > 0 && starters.contains(&trimmed[..word_end])
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

    /// Like `get_last_word`, but keeps internal `.`s so multi-dot
    /// abbreviations (`w.e.f`, `U.S`, `p.m`) are returned whole. Splits only
    /// on whitespace and `/`. Used by abbreviation lookup so the full token
    /// can be matched against the abbreviation table.
    fn get_last_word_full<'a>(&self, text: &'a str) -> &'a str {
        let trimmed = text.trim_end();
        match trimmed
            .char_indices()
            .rfind(|(_, c)| c.is_whitespace() || *c == '/')
        {
            Some((i, c)) => &trimmed[i + c.len_utf8()..],
            None => trimmed,
        }
    }

    /// One-way override that lets `find_boundary` keep a sentence boundary
    /// even when the abbreviation / name-initial path would otherwise
    /// suppress it. Fires when the next word is a registered sentence
    /// starter and one of the following holds:
    ///
    /// - The trailing letter is uppercase. Covers single initials (`I.`),
    ///   capitalized names (`Penn.`), and all-caps acronyms (`BART.`).
    /// - The full trailing token is a known multi-dot abbreviation
    ///   (`w.e.f.`).
    /// - The trailing token is a lowercase multi-character entry that
    ///   appears verbatim in the abbreviations list (`etc.`, `feat.`,
    ///   `man.`). Single-letter lowercase tails (`a.`, `i.`) are
    ///   excluded — those only match the abbreviations list via
    ///   case-folding to a single-letter capital abbreviation, and they
    ///   commonly occur as inline list markers.
    ///
    /// Time-abbrev cases (`p.m.`, `a.m.`) never reach this branch — they
    /// bypass abbreviation handling earlier via `bypass_abbrev`.
    fn should_override_abbrev_suppression(
        &self,
        head: &str,
        last_word: &str,
        next_word_approx: &str,
    ) -> bool {
        if !self.next_word_is_sentence_starter(next_word_approx) {
            return false;
        }
        let tail_starts_uppercase = last_word
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        if tail_starts_uppercase {
            return true;
        }
        if self.is_multi_dot_abbreviation(head, last_word.len()) {
            return true;
        }
        last_word.chars().count() > 1 && self.get_abbreviations().contains(last_word)
    }

    /// True when the terminator at `[start, end)` looks like a confident
    /// sentence end: a single `.` whose preceding word is plain (not an
    /// abbreviation, not a single-letter initial) and whose follower starts
    /// with a registered sentence-starter word. Used as a structural escape
    /// valve for symmetric-pair quote ranges (`''…''`, `'…'`, `"…"`) that the
    /// non-greedy `QUOTES_REGEX` may have mispaired across a real boundary.
    fn has_strong_sentence_break(&self, paragraph: &str, start: usize, end: usize) -> bool {
        if &paragraph[start..end] != "." {
            return false;
        }
        let head = &paragraph[..start];
        let last_word = self.get_last_word(head);
        if last_word.is_empty() || is_single_ascii_upper(last_word) {
            return false;
        }
        if self.is_abbreviation(head, last_word, ".")
            || self.is_multi_dot_abbreviation(head, last_word.len())
        {
            return false;
        }
        let next_index = paragraph.ceil_char_boundary(start + 1);
        let next_word_approx = self.get_next_word_approx(paragraph, next_index);
        // `closer + . + UpperWord` (e.g. `'' . Lead`) is a structurally strong
        // sentence break: a symmetric quote closer immediately before a
        // terminator marks the end of a quoted sentence, and a capitalized
        // follower starts the next. Accept it without requiring a registered
        // starter word — the starter list can't enumerate every proper noun.
        if is_symmetric_quote_closer(last_word) {
            let trimmed = next_word_approx.trim_start();
            if trimmed
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
            {
                return true;
            }
        }
        self.next_word_is_sentence_starter(next_word_approx)
    }

    /// True when `head`'s trailing token is a multi-dot abbreviation listed
    /// in this language's abbreviation table (`w.e.f`, `U.S.`, ...). The
    /// full dotted token is used for the lookup, not just the post-`.` tail
    /// `get_last_word` returns. `tail_len = get_last_word(head).len()`
    /// serves as an O(1) "the full token contains a `.`" check: when the
    /// full token is no longer than the tail there is no internal dot and
    /// the lookup is skipped along with its case-folding allocations.
    fn is_multi_dot_abbreviation(&self, head: &str, tail_len: usize) -> bool {
        let last_word_full = self.get_last_word_full(head);
        if last_word_full.len() <= tail_len {
            return false;
        }
        let abbrevs = self.get_abbreviations();
        abbrevs.contains(last_word_full)
            || abbrevs.contains(last_word_full.to_lowercase().as_str())
            || abbrevs.contains(last_word_full.to_uppercase().as_str())
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

        // Emphatic `!`/`?` followed (optionally through a symmetric-quote
        // token like `'`, `''`, `"`) by a lowercase/digit word is a
        // mid-sentence continuation, not a sentence end. Covers two shapes:
        //   * free-standing terminator: `Father Came Too ! is a British …`
        //     and `... All Grown Up ! '' and adult voices ...` (orphan `''`
        //     between the `!` and the real continuation).
        //   * glued terminator inside a closing quote: `… eh, boy? ' he said,
        //     …` where `?` ends a quoted utterance but the surrounding
        //     sentence continues with a lowercase reporting clause.
        // Single-byte equality on `matched` implicitly excludes multi-char
        // runs, ellipses, and `.`.
        let is_emphatic = matched == "!" || matched == "?";
        if is_emphatic && continuation_after_orphan_quote(next_word_approx) {
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

        // Bypass abbrev/name-initial suppression after `a.m./p.m.` (any case)
        // when the follower starts a new clock reading (digit) or a new
        // sentence (capital with a lowercase introducing preposition like
        // `at 6 p.m. Mr.`). Fronted `At 5 a.m. Mr.` keeps the suppression so
        // the time phrase reads as adverbial.
        let head_trimmed = head.trim_end();
        let ends_with_time_abbrev = {
            let mut it = head_trimmed.bytes().rev();
            match (it.next(), it.next(), it.next()) {
                (Some(c2), Some(b'.'), Some(c0)) => {
                    matches!(c0.to_ascii_lowercase(), b'a' | b'p')
                        && c2.to_ascii_lowercase() == b'm'
                }
                _ => false,
            }
        };
        
        let bypass_abbrev = matched == "."
            && ends_with_time_abbrev
            && {
                let mut chars = next_word_approx.trim_start().chars();
                match chars.next() {
                    Some(c) if c.is_ascii_digit() => true,
                    Some(c) if c.is_uppercase() => !time_phrase_is_fronted(head_trimmed),
                    _ => false,
                }
            };

        if !bypass_abbrev {
            let last_word = self.get_last_word(head);
            if matched == "." {
                let suppressed = (is_single_ascii_upper(last_word)
                    && self.is_name_initial(head, last_word, next_word_approx))
                    || self.is_abbreviation(head, last_word, &text[start..end]);

                if suppressed
                    && !self.should_override_abbrev_suppression(
                        head,
                        last_word,
                        next_word_approx,
                    )
                {
                    return None;
                }
            } else if self.is_abbreviation(head, last_word, &text[start..end]) {
                return None;
            }
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

        let push_regex_ranges = |regex: &Regex,
                                 kind: SkippableRangeType,
                                 out: &mut Vec<SkippableRange>| {
            for mat in regex.find_iter(text) {
                out.push(SkippableRange::new(mat.start(), mat.end(), kind));
            }
        };

        push_regex_ranges(&QUOTES_REGEX, SkippableRangeType::Quote, &mut skippable_ranges);
        append_space_padded_quote_pairs(text, &mut skippable_ranges);
        push_regex_ranges(&PARENS_REGEX, SkippableRangeType::Parentheses, &mut skippable_ranges);
        push_regex_ranges(&EMAIL_REGEX, SkippableRangeType::Email, &mut skippable_ranges);

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
