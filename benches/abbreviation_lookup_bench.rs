use criterion::{Criterion, criterion_group, criterion_main};
use std::collections::HashSet;
use std::hint::black_box;

const ABBREV_SOURCE: &str = include_str!("../src/languages/abbrev/en.txt");

fn load_abbrevs() -> Vec<String> {
    ABBREV_SOURCE
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.starts_with("//") && !line.is_empty())
        .collect()
}

const QUERIES: &[&str] = &[
    // hits (varied case to exercise the orig/lower/upper fallbacks)
    "Dr", "Prof", "Mr", "Mrs", "Ph", "Jan", "etc", "U.S",
    // misses
    "Smith", "Johnson", "meeting", "embassy", "discussed", "lasted",
    "Brown", "Davis", "there", "program", "until", "with",
];

fn vec_contains_3case(abbrevs: &[String], word: &str) -> bool {
    abbrevs.contains(&word.to_string())
        || abbrevs.contains(&word.to_lowercase())
        || abbrevs.contains(&word.to_uppercase())
}

fn hashset_contains_3case(abbrevs: &HashSet<String>, word: &str) -> bool {
    abbrevs.contains(word)
        || abbrevs.contains(word.to_lowercase().as_str())
        || abbrevs.contains(word.to_uppercase().as_str())
}

fn bench_membership(c: &mut Criterion) {
    let vec_abbrevs: Vec<String> = load_abbrevs();
    let hash_abbrevs: HashSet<String> = load_abbrevs().into_iter().collect();

    let mut group = c.benchmark_group("abbrev_membership");

    group.bench_function("vec_contains", |b| {
        b.iter(|| {
            let mut hits = 0u32;
            for q in QUERIES {
                if vec_contains_3case(black_box(&vec_abbrevs), black_box(q)) {
                    hits += 1;
                }
            }
            hits
        })
    });

    group.bench_function("hashset_contains", |b| {
        b.iter(|| {
            let mut hits = 0u32;
            for q in QUERIES {
                if hashset_contains_3case(black_box(&hash_abbrevs), black_box(q)) {
                    hits += 1;
                }
            }
            hits
        })
    });

    group.finish();
}

fn bench_membership_realistic(c: &mut Criterion) {
    let text = "Dr. Smith met with Prof. Johnson at the U.S. embassy. \
                They discussed the Ph.D. program at MIT. \
                The meeting was at 3 p.m. in Jan. and lasted until 5 p.m. \
                Mr. Brown and Mrs. Davis were also there.";
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == '.' || c == ',')
        .filter(|w| !w.is_empty())
        .collect();

    let vec_abbrevs: Vec<String> = load_abbrevs();
    let hash_abbrevs: HashSet<String> = load_abbrevs().into_iter().collect();

    let mut group = c.benchmark_group("abbrev_membership_realistic");

    group.bench_function("vec_contains", |b| {
        b.iter(|| {
            let mut hits = 0u32;
            for w in &words {
                if vec_contains_3case(black_box(&vec_abbrevs), black_box(w)) {
                    hits += 1;
                }
            }
            hits
        })
    });

    group.bench_function("hashset_contains", |b| {
        b.iter(|| {
            let mut hits = 0u32;
            for w in &words {
                if hashset_contains_3case(black_box(&hash_abbrevs), black_box(w)) {
                    hits += 1;
                }
            }
            hits
        })
    });

    group.finish();
}

criterion_group!(benches, bench_membership, bench_membership_realistic);
criterion_main!(benches);
