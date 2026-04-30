// Microbenchmark: split_inclusive('\n') vs memchr::memchr_iter for line iteration
// in the list-item detector.
//
// Both implementations share the same classify_line + matchers logic so the
// only thing being measured is the line-iteration strategy. Tests cover three
// scenarios: list-heavy (every line is a marker), list-free prose (the common
// case where the scanner finds nothing), and mixed (a realistic markdown
// document with bullets and prose interleaved).

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

// ---- shared classifier (identical to production) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerFamily {
    Tier1,
    Bullet,
    Numeric,
    LetterParen,
    LetterDot,
    Roman,
}

const FAMILY_COUNT: usize = 6;

impl MarkerFamily {
    fn is_tier1(self) -> bool {
        matches!(self, Self::Tier1)
    }
    fn is_tier2(self) -> bool {
        !self.is_tier1()
    }
    fn idx(self) -> usize {
        self as usize
    }
}

const UNICODE_BULLETS: &[char] = &[
    '•', '◦', '▪', '▫', '■', '□', '●', '○', '⁃', '⁌', '⁍', '◆', '◇', '★', '☆', '➤', '➢', '➣', '▶',
    '▸', '►',
];

fn classify_line(line: &str) -> Option<MarkerFamily> {
    let after_indent = skip_horiz_ws(line);
    let (family, marker_len) = consume_marker(after_indent)?;
    let after_marker = &after_indent[marker_len..];
    let after_spaces = skip_horiz_ws(after_marker);
    let had_space = after_marker.len() != after_spaces.len();
    let has_content = matches!(after_spaces.chars().next(), Some(c) if c != '\n' && c != '\r');
    (had_space && has_content).then_some(family)
}

fn consume_marker(s: &str) -> Option<(MarkerFamily, usize)> {
    None.or_else(|| match_unicode_bullet(s).map(|n| (MarkerFamily::Tier1, n)))
        .or_else(|| match_paren_form(s).map(|n| (MarkerFamily::Tier1, n)))
        .or_else(|| match_roman(s).map(|n| (MarkerFamily::Roman, n)))
        .or_else(|| match_numeric(s).map(|n| (MarkerFamily::Numeric, n)))
        .or_else(|| match_ascii_bullet(s).map(|n| (MarkerFamily::Bullet, n)))
        .or_else(|| match_letter_paren(s).map(|n| (MarkerFamily::LetterParen, n)))
        .or_else(|| match_letter_dot(s).map(|n| (MarkerFamily::LetterDot, n)))
}

fn match_unicode_bullet(s: &str) -> Option<usize> {
    let c = s.chars().next()?;
    UNICODE_BULLETS.contains(&c).then(|| c.len_utf8())
}
fn match_ascii_bullet(s: &str) -> Option<usize> {
    match s.as_bytes().first()? {
        b'*' | b'+' | b'-' => Some(1),
        _ if s.starts_with('–') => Some('–'.len_utf8()),
        _ if s.starts_with('—') => Some('—'.len_utf8()),
        _ => None,
    }
}
fn match_numeric(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let n = b.iter().take_while(|c| c.is_ascii_digit()).count();
    (n > 0 && matches!(b.get(n), Some(b'.' | b')'))).then_some(n + 1)
}
fn match_roman(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let n = b.iter().take_while(|&&c| is_roman_byte(c)).count();
    (n >= 2 && matches!(b.get(n), Some(b'.' | b')'))).then_some(n + 1)
}
fn match_letter_paren(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b')').then_some(2)
}
fn match_letter_dot(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    (b.len() >= 2 && b[0].is_ascii_lowercase() && b[1] == b'.').then_some(2)
}
fn match_paren_form(s: &str) -> Option<usize> {
    let inside = s.strip_prefix('(')?;
    let close = inside.find(')')?;
    let inner = inside[..close].trim_matches(|c: char| matches!(c, ' ' | '\t'));
    let valid = !inner.is_empty()
        && (inner.bytes().all(|b| b.is_ascii_digit())
            || (inner.len() == 1 && inner.as_bytes()[0].is_ascii_alphabetic())
            || inner.bytes().all(is_roman_byte));
    valid.then_some(close + 2)
}
fn skip_horiz_ws(s: &str) -> &str {
    s.trim_start_matches(|c: char| matches!(c, ' ' | '\t'))
}
fn is_roman_byte(b: u8) -> bool {
    matches!(
        b.to_ascii_lowercase(),
        b'i' | b'v' | b'x' | b'l' | b'c' | b'd' | b'm'
    )
}

fn finalise(mut candidates: Vec<(usize, MarkerFamily)>) -> Vec<usize> {
    let mut counts = [0u32; FAMILY_COUNT];
    for &(_, f) in &candidates {
        if f.is_tier2() {
            counts[f.idx()] += 1;
        }
    }
    candidates.retain(|&(_, f)| f.is_tier1() || counts[f.idx()] >= 2);
    candidates.into_iter().map(|(p, _)| p).collect()
}

// ---- candidate A: split_inclusive ----

fn detect_split_inclusive(paragraph: &str) -> Vec<usize> {
    let mut candidates: Vec<(usize, MarkerFamily)> = Vec::new();
    let mut offset = 0;
    for line in paragraph.split_inclusive('\n') {
        if let Some(family) = classify_line(line) {
            candidates.push((offset, family));
        }
        offset += line.len();
    }
    finalise(candidates)
}

// ---- candidate B: memchr-driven line iteration ----

fn detect_memchr(paragraph: &str) -> Vec<usize> {
    let bytes = paragraph.as_bytes();
    let mut candidates: Vec<(usize, MarkerFamily)> = Vec::new();
    let mut line_start = 0usize;

    for nl_pos in memchr::memchr_iter(b'\n', bytes) {
        // Slice goes through nl_pos inclusive; classify_line ignores the trailing \n.
        let line = &paragraph[line_start..=nl_pos];
        if let Some(family) = classify_line(line) {
            candidates.push((line_start, family));
        }
        line_start = nl_pos + 1;
    }
    // Trailing line with no \n
    if line_start < bytes.len() {
        let line = &paragraph[line_start..];
        if let Some(family) = classify_line(line) {
            candidates.push((line_start, family));
        }
    }

    finalise(candidates)
}

// ---- fixtures ----

fn list_heavy(target_size: usize) -> String {
    // A realistic markdown-style list-heavy document.
    let unit = "* First item with some descriptive text about it.\n\
                * Second item with more text and a period. And another sentence.\n\
                * Third item is shorter.\n\
                1. Numbered item one.\n\
                2. Numbered item two.\n\
                3. Numbered item three.\n\
                (a) Lettered first.\n\
                (b) Lettered second.\n\
                • Unicode bullet item.\n\
                • Another unicode bullet.\n\n";
    let mut out = String::with_capacity(target_size + unit.len());
    while out.len() < target_size {
        out.push_str(unit);
    }
    out
}

fn list_free(target_size: usize) -> String {
    // Plain prose, no list markers anywhere. Newlines at sentence boundaries
    // simulate hard-wrapped prose (worst case for any per-line scan).
    let unit = "This is a sentence in a paragraph.\n\
                Here is another sentence on a new line, with no list marker.\n\
                Dr. Smith said the result was significant.\n\
                The meeting was at 3 p.m. on Jan. 15th in Boston.\n\
                She replied, \"That is correct.\" Then she walked away.\n\n";
    let mut out = String::with_capacity(target_size + unit.len());
    while out.len() < target_size {
        out.push_str(unit);
    }
    out
}

fn mixed(target_size: usize) -> String {
    // Intro prose, a small list, more prose. The realistic shape.
    let unit = "Introduction paragraph with a couple of sentences. \
                Followed by another sentence to make it realistic.\n\n\
                Here are the rules:\n\
                * Be kind to others.\n\
                * Be honest in your work.\n\
                * Verify your assumptions.\n\n\
                After the list, more prose continues. \
                The conclusion ties everything together neatly.\n\n";
    let mut out = String::with_capacity(target_size + unit.len());
    while out.len() < target_size {
        out.push_str(unit);
    }
    out
}

fn cjk_no_lists(target_size: usize) -> String {
    // Multi-byte UTF-8 worst case: Japanese prose with no list markers.
    // split_inclusive must walk char boundaries; memchr just scans for b'\n'.
    let unit = "これは日本語の文です。もう一つあります。そしてもう一つあります。\n\
                これは別の段落です。ここでも何かを書きます。\n\n";
    let mut out = String::with_capacity(target_size + unit.len());
    while out.len() < target_size {
        out.push_str(unit);
    }
    out
}

// ---- benches ----

fn bench_implementations(c: &mut Criterion) {
    let scenarios: Vec<(&str, fn(usize) -> String)> = vec![
        ("list_heavy", list_heavy),
        ("list_free", list_free),
        ("mixed", mixed),
        ("cjk_no_lists", cjk_no_lists),
    ];

    for size in [1_000usize, 10_000, 100_000] {
        for &(name, make) in &scenarios {
            let text = make(size);
            let mut group = c.benchmark_group(format!("list_markers/{name}"));
            group.throughput(Throughput::Bytes(text.len() as u64));

            group.bench_with_input(BenchmarkId::new("split_inclusive", size), &text, |b, t| {
                b.iter(|| black_box(detect_split_inclusive(black_box(t))))
            });
            group.bench_with_input(BenchmarkId::new("memchr", size), &text, |b, t| {
                b.iter(|| black_box(detect_memchr(black_box(t))))
            });

            group.finish();
        }
    }
}

criterion_group!(benches, bench_implementations);
criterion_main!(benches);
