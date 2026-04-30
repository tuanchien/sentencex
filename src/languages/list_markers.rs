// List-item line/inline detector.
//
// Scans each paragraph once via memchr-driven line iteration, classifying
// markers at both line starts and inline positions (after a whitespace run).
// A "list = a sequence" sibling rule with a single winning family per paragraph
// keeps false positives down. The caller uses the returned offsets to emit
// sentence boundaries and to mark each item span as `SkippableRange::ListItem`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerFamily {
    Tier1,       // unicode bullets, parenthesised forms — fire on a single line-start match
    Bullet,      // * + - – —
    Numeric,     // 1. 1) 1.) 23.
    LetterParen, // a) A) a.) A.)
    LetterDot,   // a. b. (lowercase only)
    Roman,       // ii. iii) ii.) (≥2 chars, plus single-letter promoted via sibling rule)
}

const FAMILY_COUNT: usize = 6;

impl MarkerFamily {
    fn idx(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    pos: usize,
    family: MarkerFamily,
    first_byte: u8,
    line_start: bool,
}

const MIN_GAP_BYTES: usize = 4;

/// Returns byte offsets (within `paragraph`) of accepted list-item starts,
/// in source order with no duplicates.
pub(crate) fn detect_list_items(paragraph: &str) -> Vec<usize> {
    let bytes = paragraph.as_bytes();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut line_start = 0usize;

    for nl_pos in memchr::memchr_iter(b'\n', bytes) {
        scan_line(paragraph, line_start, nl_pos + 1, &mut candidates);
        line_start = nl_pos + 1;
    }
    if line_start < bytes.len() {
        scan_line(paragraph, line_start, bytes.len(), &mut candidates);
    }

    finalise(candidates, paragraph)
}

fn scan_line(text: &str, line_start: usize, line_end: usize, out: &mut Vec<Candidate>) {
    let line = &text[line_start..line_end];

    // Line-start classification (handles indentation).
    if let Some((family, _marker_len, first_byte)) = classify_line(line) {
        out.push(Candidate {
            pos: line_start,
            family,
            first_byte,
            line_start: true,
        });
    }

    // Inline scan: skip past leading indent, then look for whitespace-preceded
    // markers within the line content.
    let bytes = text.as_bytes();
    let mut i = line_start;
    while i < line_end && is_horiz_ws(bytes[i]) {
        i += 1;
    }

    while i < line_end {
        if is_horiz_ws(bytes[i]) {
            let mut ws_end = i + 1;
            while ws_end < line_end && is_horiz_ws(bytes[ws_end]) {
                ws_end += 1;
            }
            if ws_end < line_end && bytes[ws_end] != b'\n' && bytes[ws_end] != b'\r' {
                if let Some((family, _marker_len, first_byte)) =
                    classify_marker_at(&text[ws_end..line_end])
                {
                    out.push(Candidate {
                        pos: ws_end,
                        family,
                        first_byte,
                        line_start: false,
                    });
                }
            }
            i = ws_end;
        } else {
            i += 1;
        }
    }
}

fn classify_line(line: &str) -> Option<(MarkerFamily, usize, u8)> {
    let after_indent = skip_horiz_ws(line);
    let first_byte = *after_indent.as_bytes().first()?;
    let (family, marker_len) = consume_marker(after_indent)?;
    let _next = next_content_char(&after_indent[marker_len..])?;
    Some((family, marker_len, first_byte))
}

fn classify_marker_at(s: &str) -> Option<(MarkerFamily, usize, u8)> {
    let first_byte = *s.as_bytes().first()?;
    let (family, marker_len) = consume_marker(s)?;

    // Inline rule (1): reject bare-dot closers (`1.`, `a.`, `ii.`). These
    // collide too strongly with sentence-ending periods in prose — without
    // semantic context the case `Foo 1. The first item. 2. The second item.`
    // is indistinguishable from a wrapped sentence ending in "...Foo 1." plus
    // "The first item." plus "...item 2." plus "The second item." Inline lists
    // must use `)` or `.)` closers, or Tier 1 markers. Line-start markers are
    // unaffected (the line break itself is the structural signal).
    let last_byte = s.as_bytes()[marker_len - 1];
    let bare_dot_closer = last_byte == b'.'
        && matches!(
            family,
            MarkerFamily::Numeric | MarkerFamily::LetterDot | MarkerFamily::Roman
        );
    if bare_dot_closer {
        return None;
    }

    // Inline rule (2): reject if next content char is a lowercase letter.
    // Defends against ordinal date forms in Finnish/Russian/etc.
    // (`1. kesäkuuta`, `1. января`) where the terminator regex would itself
    // suppress the boundary via `continue_in_next_word`. Line-start markers
    // skip this rule because lowercase items there are unambiguous
    // (`1. apples\n2. oranges`).
    let next = next_content_char(&s[marker_len..])?;
    if next.is_lowercase() {
        return None;
    }

    // Inline rule (3): reject ASCII bullets (`*`, `+`, `-`) entirely. These
    // have no closing punctuation, so without line-start context they are
    // indistinguishable from parenthetical or hyphen uses ("Su - 24",
    // "fast - track"). Two such uses in a paragraph would otherwise satisfy
    // the sibling rule and emit false boundaries. Line-start bullets are
    // unaffected (they go through classify_line, not classify_marker_at).
    // Symmetric with the en/em-dash exclusion in match_ascii_bullet.
    if family == MarkerFamily::Bullet {
        return None;
    }

    Some((family, marker_len, first_byte))
}

/// Returns the first non-space, non-tab character after `after_marker`, only
/// if at least one space/tab is present and the resulting character is real
/// content (not a line terminator).
fn next_content_char(after_marker: &str) -> Option<char> {
    let after_spaces = skip_horiz_ws(after_marker);
    if after_marker.len() == after_spaces.len() {
        return None;
    }
    let c = after_spaces.chars().next()?;
    (c != '\n' && c != '\r').then_some(c)
}

fn consume_marker(s: &str) -> Option<(MarkerFamily, usize)> {
    // Order encodes priority. Tier 1 first; multi-char roman before single-letter
    // `[a-z]\.` so `ii.` isn't classified as `LetterDot`.
    None.or_else(|| match_unicode_bullet(s).map(|n| (MarkerFamily::Tier1, n)))
        .or_else(|| match_paren_form(s).map(|n| (MarkerFamily::Tier1, n)))
        .or_else(|| match_roman(s).map(|n| (MarkerFamily::Roman, n)))
        .or_else(|| match_numeric(s).map(|n| (MarkerFamily::Numeric, n)))
        .or_else(|| match_ascii_bullet(s).map(|n| (MarkerFamily::Bullet, n)))
        .or_else(|| match_letter_paren(s).map(|n| (MarkerFamily::LetterParen, n)))
        .or_else(|| match_letter_dot(s).map(|n| (MarkerFamily::LetterDot, n)))
}

const UNICODE_BULLETS: &[char] = &[
    '•', '◦', '▪', '▫', '■', '□', '●', '○', '⁃', '⁌', '⁍', '◆', '◇', '★', '☆', '➤', '➢', '➣', '▶',
    '▸', '►',
];

fn match_unicode_bullet(s: &str) -> Option<usize> {
    let c = s.chars().next()?;
    UNICODE_BULLETS.contains(&c).then(|| c.len_utf8())
}

fn match_ascii_bullet(s: &str) -> Option<usize> {
    // En/em dashes (`–`/`—`) are intentionally NOT included: they collide with
    // parenthetical dashes in prose (e.g. Kazakh "(1905 — 11)") where the
    // sibling rule would falsely activate a list. Real `–`/`—` line-start
    // bullets are rare; users wanting them can use `*` or `-` instead.
    match s.as_bytes().first()? {
        b'*' | b'+' | b'-' => Some(1),
        _ => None,
    }
}

fn match_numeric(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let n = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if n == 0 {
        return None;
    }
    closer_len_at(b, n).map(|cl| n + cl)
}

fn match_roman(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let n = b.iter().take_while(|&&c| is_roman_byte(c)).count();
    if n < 2 {
        return None;
    }
    closer_len_at(b, n).map(|cl| n + cl)
}

fn match_letter_paren(s: &str) -> Option<usize> {
    // Accepts `a)` and `a.)` but not `a.` alone (that's LetterDot's territory).
    let b = s.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_alphabetic() {
        return None;
    }
    match b[1] {
        b')' => Some(2),
        b'.' if b.get(2) == Some(&b')') => Some(3),
        _ => None,
    }
}

fn match_letter_dot(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_lowercase() && b[1] == b'.').then_some(2)
}

fn match_paren_form(s: &str) -> Option<usize> {
    // (1), (12), (a), (A), (ii), (iv) — short, unpadded inners only.
    // Padded inners (`( 1894 )`) and 3+ digit inners (`(1894)`) are prose
    // year/date citations, not list markers.
    let inside = s.strip_prefix('(')?;
    let close = inside.find(')')?;
    let inner = inside[..close].as_bytes();

    let short_numeric = (1..=2).contains(&inner.len()) && inner.iter().all(|b| b.is_ascii_digit());
    let single_letter = inner.len() == 1 && inner[0].is_ascii_alphabetic();
    let roman = !inner.is_empty() && inner.iter().all(|&b| is_roman_byte(b));

    (short_numeric || single_letter || roman).then_some(close + 2)
}

/// Length of marker closer at byte position `pos` — `.`, `)`, or `.)`.
fn closer_len_at(b: &[u8], pos: usize) -> Option<usize> {
    match b.get(pos)? {
        b'.' if b.get(pos + 1) == Some(&b')') => Some(2),
        b'.' | b')' => Some(1),
        _ => None,
    }
}

fn skip_horiz_ws(s: &str) -> &str {
    s.trim_start_matches(|c: char| matches!(c, ' ' | '\t'))
}

fn is_horiz_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

fn is_roman_byte(b: u8) -> bool {
    matches!(
        b.to_ascii_lowercase(),
        b'i' | b'v' | b'x' | b'l' | b'c' | b'd' | b'm'
    )
}

fn finalise(mut candidates: Vec<Candidate>, _paragraph: &str) -> Vec<usize> {
    if candidates.is_empty() {
        return Vec::new();
    }

    // Sort by position once. Where a line-start and inline candidate share a
    // position (rare; possible only on degenerate inputs), prefer line-start.
    candidates.sort_by(|a, b| a.pos.cmp(&b.pos).then(b.line_start.cmp(&a.line_start)));
    candidates.dedup_by_key(|c| c.pos);

    // Promotion: if any multi-char Roman exists, single-letter roman-shaped
    // LetterDot/LetterParen candidates join the Roman family so they count
    // as siblings (handles `i.) ... ii.)` style).
    let has_multi_roman = candidates.iter().any(|c| c.family == MarkerFamily::Roman);
    if has_multi_roman {
        for c in &mut candidates {
            if matches!(
                c.family,
                MarkerFamily::LetterDot | MarkerFamily::LetterParen
            ) && is_roman_byte(c.first_byte)
            {
                c.family = MarkerFamily::Roman;
            }
        }
    }

    // Per-family gap pruning. Drop candidates too close to the previous match
    // *of the same family*. A real list item carries content; `e. e. cummings`
    // (LetterDot gap 3) does not. We do this BEFORE family selection so the
    // sibling counts reflect the post-pruning state.
    let mut pruned: Vec<Candidate> = Vec::with_capacity(candidates.len());
    let mut family_last = [usize::MAX; FAMILY_COUNT];
    for c in candidates {
        let last = family_last[c.family.idx()];
        if last == usize::MAX || c.pos - last >= MIN_GAP_BYTES {
            family_last[c.family.idx()] = c.pos;
            pruned.push(c);
        }
    }
    let candidates = pruned;

    // Determine winning family.
    let mut counts = [0u32; FAMILY_COUNT];
    for c in &candidates {
        counts[c.family.idx()] += 1;
    }

    let tier1_count = counts[MarkerFamily::Tier1.idx()];
    let tier1_at_line_start = candidates
        .iter()
        .any(|c| c.family == MarkerFamily::Tier1 && c.line_start);
    let tier1_wins = tier1_at_line_start || tier1_count >= 2;

    let winner = if tier1_wins {
        Some(MarkerFamily::Tier1)
    } else {
        const TIER2: &[MarkerFamily] = &[
            MarkerFamily::Bullet,
            MarkerFamily::Numeric,
            MarkerFamily::LetterParen,
            MarkerFamily::LetterDot,
            MarkerFamily::Roman,
        ];
        let mut best: Option<(MarkerFamily, u32, usize)> = None;
        for &f in TIER2 {
            let n = counts[f.idx()];
            if n < 2 {
                continue;
            }
            let first_pos = candidates
                .iter()
                .find(|c| c.family == f)
                .map(|c| c.pos)
                .unwrap();
            match best {
                None => best = Some((f, n, first_pos)),
                Some((_, bn, bp)) if n > bn || (n == bn && first_pos < bp) => {
                    best = Some((f, n, first_pos));
                }
                _ => {}
            }
        }
        best.map(|(f, _, _)| f)
    };

    let Some(winner) = winner else {
        return Vec::new();
    };

    candidates
        .into_iter()
        .filter(|c| c.family == winner)
        .map(|c| c.pos)
        .collect()
}

#[cfg(test)]
mod fixture {
    use crate::languages::English;
    use crate::languages::tests::run_language_tests;

    #[test]
    fn segments_lists() {
        run_language_tests(English {}, "tests/lists.txt");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(s: &str) -> Vec<usize> {
        detect_list_items(s)
    }

    // Line-start cases (existing behaviour, preserved).

    #[test]
    fn line_start_unicode_bullets() {
        // `•` is 3 UTF-8 bytes, so "• apples\n" is 11 bytes.
        assert_eq!(detect("• apples\n• oranges\n"), vec![0, 11]);
    }

    #[test]
    fn line_start_ascii_bullets() {
        assert_eq!(detect("* one\n* two\n* three\n"), vec![0, 6, 12]);
    }

    #[test]
    fn line_start_numeric() {
        assert_eq!(detect("1. apples\n2) oranges\n"), vec![0, 10]);
    }

    #[test]
    fn line_start_roman() {
        assert_eq!(detect("ii. First\niii. Second\n"), vec![0, 10]);
    }

    #[test]
    fn line_start_paren_form_single() {
        // Tier 1 fires on a single line-start match.
        assert_eq!(detect("(a) only one item here\n"), vec![0]);
    }

    #[test]
    fn line_start_intro_then_bullets() {
        assert_eq!(
            detect("Here are the rules:\n* Be kind.\n* Be honest.\n"),
            vec![20, 31]
        );
    }

    // Inline cases (the new failure cases the user supplied).

    #[test]
    fn inline_numeric_dot_paren() {
        // 1.) ... 2.)
        let starts = detect("1.) The first item. 2.) The second item.");
        assert_eq!(starts, vec![0, 20]);
    }

    #[test]
    fn inline_numeric_paren_only() {
        // 1) ... 2) (no dots)
        let starts = detect("1) The first item 2) The second item");
        assert_eq!(starts, vec![0, 18]);
    }

    #[test]
    fn inline_numeric_dot_only_not_handled() {
        // Deliberately NOT split: bare-dot closer (`1.` / `2.`) inline is
        // ambiguous with a wrapped prose sentence ending in `...Foo 1.`.
        // Without semantic context we err on the side of not over-splitting.
        let starts = detect("1. The first item. 2. The second item.");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn inline_letter_dot_not_handled() {
        // Same reasoning: bare-dot closer + lowercase letter is a common
        // initials shape (e. e. cummings) and a common date/abbrev shape.
        let starts = detect("a. The first item b. The second item");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn inline_roman_dot_not_handled() {
        let starts = detect("ii. The first item iii. The second item");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn inline_unicode_bullet_with_decoration() {
        // • 9. ... • 10. — bullets are the items, numerics are decoration.
        let s = "• 9. The first item • 10. The second item";
        let starts = detect(s);
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[0], 0);
        assert_eq!(&s[starts[1]..starts[1] + 3], "•");
    }

    #[test]
    fn inline_year_parens_not_a_list() {
        let s = "Examples include 'Sonar Tari' ( 1894 ), 'Chitra' ( 1896 ), \
                 and 'Katha O Kahini' ( 1900 ).";
        assert!(detect(s).is_empty(), "got {:?}", detect(s));
        let s2 = "Works (1894), (1896), and (1900) are notable.";
        assert!(detect(s2).is_empty(), "got {:?}", detect(s2));
    }

    #[test]
    fn inline_letter_dot_paren() {
        // a.) ... b.)
        let starts = detect("a.) The first item b.) The second item");
        assert_eq!(starts, vec![0, 19]);
    }

    #[test]
    fn inline_letter_paren_only() {
        // a) ... b)
        let starts = detect("a) The first item b) The second item");
        assert_eq!(starts, vec![0, 18]);
    }

    #[test]
    fn inline_roman_dot_paren_with_promotion() {
        // i.) ... ii.) — single-letter `i` promoted to Roman because `ii` is Roman.
        let starts = detect("i.) The first item ii.) The second item");
        assert_eq!(starts, vec![0, 19]);
    }

    // Gap-pruning safety.

    #[test]
    fn ee_cummings_no_segmentation() {
        // `e. e.` has zero non-marker content between markers → drop the second.
        let starts = detect("From\ne. e. cummings, with love.");
        // No siblings survive after gap pruning, so the LetterDot ≥2 rule fails.
        assert!(starts.is_empty(), "got {starts:?}");
    }

    // Existing safety contract.

    #[test]
    fn uppercase_letter_dot_excluded() {
        let starts = detect("Reviewed by\nA. Smith and\nB. Jones.");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn lone_lowercase_letter_dot_no_siblings() {
        let starts = detect("The answer is a.\nFollow up later.");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn lone_numeric_after_wrap() {
        let starts = detect("The total was\n1. Two hundred dollars exactly.");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn eg_at_line_start() {
        let starts = detect("e.g. one\ne.g. two\n");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn marker_only_lines() {
        let starts = detect("*\n*\n*\n");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn no_markers_plain_prose() {
        let starts = detect("Hello world. This is a test. Three sentences here.");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn empty_paragraph() {
        let starts = detect("");
        assert!(starts.is_empty(), "got {starts:?}");
    }

    #[test]
    fn indented_markers() {
        // Line-start classify_line skips indent; pos still points to line start.
        assert_eq!(detect("   * indented\n   * also indented\n"), vec![0, 14]);
    }
}
