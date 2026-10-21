//! `cargo bench -p ascii-core` — end-to-end conversion of synthetic photos
//! (no decode), per mode and size.

#[path = "../tests/common/mod.rs"]
mod common;

use ascii_core::{Mode, Options};
use criterion::{Criterion, criterion_group, criterion_main};

fn bench(c: &mut Criterion) {
    let atlas = common::atlas();
    // Smooth shading plus fine detail, so every pipeline stage does real work.
    let photo = |w: usize, h: usize| {
        common::image(w, h, move |x, y| {
            let (u, v) = (x / w as f32, y / h as f32);
            0.5 + 0.3 * (u * 9.0).sin() * (v * 7.0).cos() + 0.2 * ((x * 0.37).sin() * (y * 0.23).cos())
        })
    };
    for (label, w, h, cols) in [("1080p", 1920, 1080, 200), ("4k", 3840, 2160, 300)] {
        let src = photo(w, h);
        for mode in [Mode::Density, Mode::Shape] {
            let opts = Options { cols, mode, ..Options::DEFAULT };
            c.bench_function(&format!("{label} {cols} cols {mode:?}"), |b| {
                b.iter(|| ascii_core::convert(&src, &atlas, &opts).unwrap())
            });
        }
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
