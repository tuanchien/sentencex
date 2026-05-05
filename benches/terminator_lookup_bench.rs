use criterion::{Criterion, criterion_group, criterion_main};
use sentencex::constants::{GLOBAL_SENTENCE_TERMINATORS, GLOBAL_SENTENCE_TERMINATORS_SET};
use std::hint::black_box;

fn array_contains(ch: char) -> bool {
    GLOBAL_SENTENCE_TERMINATORS.contains(&ch.to_string().as_str())
}

fn hashset_contains(ch: char) -> bool {
    GLOBAL_SENTENCE_TERMINATORS_SET.contains(&ch)
}

fn bench_terminator_lookup(c: &mut Criterion) {
    let text = "Dr. Smith met with Prof. Johnson at the U.S. embassy. \
                They discussed the Ph.D. program at MIT! Was it useful? \
                The meeting was at 3 p.m. in Jan. and lasted until 5 p.m. \
                Mr. Brown and Mrs. Davis were also there. \
                日本語の文。中文的句子。Հայերեն նախադասություն։";
    let chars: Vec<char> = text.chars().collect();

    let mut group = c.benchmark_group("terminator_lookup");

    group.bench_function("array_contains", |b| {
        b.iter(|| {
            let mut hits = 0u32;
            for ch in &chars {
                if array_contains(black_box(*ch)) {
                    hits += 1;
                }
            }
            hits
        })
    });

    group.bench_function("hashset_contains", |b| {
        b.iter(|| {
            let mut hits = 0u32;
            for ch in &chars {
                if hashset_contains(black_box(*ch)) {
                    hits += 1;
                }
            }
            hits
        })
    });

    group.finish();
}

fn bench_terminator_hits_only(c: &mut Criterion) {
    let chars: Vec<char> = "!.?。｡։؟۔".chars().collect();

    let mut group = c.benchmark_group("terminator_lookup_hits_only");

    group.bench_function("array_contains", |b| {
        b.iter(|| {
            let mut hits = 0u32;
            for ch in &chars {
                if array_contains(black_box(*ch)) {
                    hits += 1;
                }
            }
            hits
        })
    });

    group.bench_function("hashset_contains", |b| {
        b.iter(|| {
            let mut hits = 0u32;
            for ch in &chars {
                if hashset_contains(black_box(*ch)) {
                    hits += 1;
                }
            }
            hits
        })
    });

    group.finish();
}

criterion_group!(benches, bench_terminator_lookup, bench_terminator_hits_only);
criterion_main!(benches);
