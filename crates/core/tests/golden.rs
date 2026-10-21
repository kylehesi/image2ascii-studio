//! Golden tests: every synthetic fixture is converted in each mode and the
//! text compared with a checked-in snapshot, so any change in output is a
//! visible, reviewed diff. Regenerate with `UPDATE_GOLDEN=1 cargo test`.
//!
//! Quality floors back the snapshots up: a change may alter output, but it
//! must not lower SSIM below the recorded floor without someone noticing.

mod common;

use std::path::PathBuf;
use std::{env, fs};

use image2ascii_core::quality::evaluate;
use image2ascii_core::{Mode, Options};

const COLS: usize = 40;

fn options(mode: Mode) -> Options {
    Options { cols: COLS, mode, ..Options::DEFAULT }
}

fn golden_path(name: &str, mode: Mode) -> PathBuf {
    let mode = format!("{mode:?}").to_lowercase();
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}_{mode}.txt"))
}

#[test]
fn outputs_match_golden_snapshots() {
    let atlas = common::atlas();
    let update = env::var_os("UPDATE_GOLDEN").is_some();
    let mut mismatches = Vec::new();
    for (name, src) in common::images() {
        for mode in [Mode::Density, Mode::Shape] {
            let text = image2ascii_core::convert(&src, &atlas, &options(mode)).unwrap().to_text();
            let path = golden_path(name, mode);
            if update {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, &text).unwrap();
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(expected) if expected.replace("\r\n", "\n") == text => {}
                Ok(_) => mismatches.push(format!("{} differs", path.display())),
                Err(_) => mismatches.push(format!("{} is missing", path.display())),
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{}\nIf the change is intended, regenerate with UPDATE_GOLDEN=1 and review the diff.",
        mismatches.join("\n")
    );
}

#[test]
fn shape_mode_beats_density_on_structured_images() {
    let atlas = common::atlas();
    for (name, src) in common::images() {
        if name == "gradient" {
            continue; // no structure to find
        }
        let score = |mode| {
            let opts = options(mode);
            let art = image2ascii_core::convert(&src, &atlas, &opts).unwrap();
            evaluate(&src, &atlas, &opts, &art).unwrap().ssim
        };
        let (density, shape) = (score(Mode::Density), score(Mode::Shape));
        assert!(shape > density, "{name}: shape {shape:.4} <= density {density:.4}");
    }
}

#[test]
fn quality_floors() {
    // (image, mode, minimum SSIM) — recorded from the implementation when
    // the snapshots were made. Raise them when quality improves.
    const FLOORS: &[(&str, Mode, f32)] = &[
        ("gradient", Mode::Density, 0.2453),
        ("gradient", Mode::Shape, 0.2453),
        ("disc", Mode::Density, 0.8803),
        ("disc", Mode::Shape, 0.9142),
        ("rings", Mode::Density, 0.1433),
        ("rings", Mode::Shape, 0.1500),
        ("diagonal", Mode::Density, 0.9275),
        ("diagonal", Mode::Shape, 0.9302),
        ("bars", Mode::Density, 0.6674),
        ("bars", Mode::Shape, 0.8578),
    ];
    let atlas = common::atlas();
    let images = common::images();
    let mut report = Vec::new();
    for &(name, mode, floor) in FLOORS {
        let src = &images.iter().find(|i| i.0 == name).unwrap().1;
        let opts = options(mode);
        let art = image2ascii_core::convert(src, &atlas, &opts).unwrap();
        let ssim = evaluate(src, &atlas, &opts, &art).unwrap().ssim;
        report.push(format!("(\"{name}\", Mode::{mode:?}, {ssim:.4}),"));
        assert!(ssim >= floor - 1e-4, "{name} {mode:?}: ssim {ssim:.4} fell below floor {floor:.4}");
    }
    eprintln!("{}", report.join("\n"));
}

#[test]
fn dithering_improves_tone_on_gradients() {
    let atlas = common::atlas();
    let src = &common::images()[0].1;
    let tone_error = |dither| {
        let opts = Options { dither, ..options(Mode::Density) };
        let art = image2ascii_core::convert(src, &atlas, &opts).unwrap();
        evaluate(src, &atlas, &opts, &art).unwrap().tone_error
    };
    let (plain, dithered) = (tone_error(0.0), tone_error(1.0));
    assert!(dithered < plain, "dithered {dithered:.4} vs plain {plain:.4}");
}

#[cfg(feature = "parallel")]
#[test]
fn parallel_matches_sequential_semantics() {
    // Every parallel stage writes disjoint outputs, so repeated runs (with
    // different thread interleavings) must agree exactly.
    let atlas = common::atlas();
    for (_, src) in common::images() {
        let first = image2ascii_core::convert(&src, &atlas, &options(Mode::Shape)).unwrap();
        for _ in 0..3 {
            assert_eq!(image2ascii_core::convert(&src, &atlas, &options(Mode::Shape)).unwrap(), first);
        }
    }
}
