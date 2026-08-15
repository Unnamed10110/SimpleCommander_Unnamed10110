//! Performance-budget benchmarks.
//!
//! Budgets (from the project plan):
//! - enumerate 100k entries: < 500 ms
//! - build_view (sort+filter) of 100k entries: < 150 ms
//! - natural_cmp throughput: > 10M cmp/s

use criterion::{criterion_group, criterion_main, Criterion};
use sc_core::entry::FsEntry;
use sc_core::sort::{build_view, natural_cmp, SortSpec};
use std::hint::black_box;

fn synth_entries(n: usize) -> Vec<FsEntry> {
    (0..n)
        .map(|i| FsEntry {
            name: format!("file_{}_{}.txt", i % 997, i),
            size: (i as u64) * 37 % 1_000_000,
            modified: i as u64,
            created: i as u64,
            attributes: if i % 7 == 0 { 0x10 } else { 0x80 },
        })
        .collect()
}

fn bench_natural_cmp(c: &mut Criterion) {
    c.bench_function("natural_cmp", |b| {
        b.iter(|| natural_cmp(black_box("file_123_abc.txt"), black_box("file_124_abc.txt")))
    });
}

fn bench_build_view_100k(c: &mut Criterion) {
    let entries = synth_entries(100_000);
    c.bench_function("build_view_100k_sorted", |b| {
        b.iter(|| build_view(black_box(&entries), SortSpec::default(), "", true))
    });
    c.bench_function("build_view_100k_filtered", |b| {
        b.iter(|| build_view(black_box(&entries), SortSpec::default(), "*_42*", true))
    });
}

fn bench_enumerate_windows_dir(c: &mut Criterion) {
    // Enumerate a real directory that exists on every Windows machine.
    c.bench_function("enumerate_system32", |b| {
        b.iter(|| {
            let mut count = 0usize;
            let _ = sc_shell::enumerate::enumerate_dir(
                std::path::Path::new("C:\\Windows\\System32"),
                |batch| {
                    count += batch.len();
                    true
                },
            );
            black_box(count)
        })
    });
}

criterion_group!(
    benches,
    bench_natural_cmp,
    bench_build_view_100k,
    bench_enumerate_windows_dir
);
criterion_main!(benches);
