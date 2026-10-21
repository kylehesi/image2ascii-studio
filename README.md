<div align="center">

# image2ascii studio

**Turn any image into high-quality ASCII art — in your browser or from the command line.**

Edge-aware character matching, font-calibrated tones, and a typewriter reveal animation
you can drop straight into a GitHub profile README.

[![CI](https://github.com/kylehesi/image2ascii-studio/actions/workflows/ci.yml/badge.svg)](https://github.com/kylehesi/image2ascii-studio/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Made with Rust](https://img.shields.io/badge/made%20with-Rust%20%2B%20WebAssembly-dea584.svg)](#how-it-works)

[**Open the web app →**](https://kylehesi.github.io/image2ascii-studio/) &nbsp;·&nbsp;
[Download the CLI](https://github.com/kylehesi/image2ascii-studio/releases/latest) &nbsp;·&nbsp;
[How it works](#how-it-works)

<!-- IMAGE SLOT: docs/images/hero.png — a photo side by side with its ASCII version.
<img src="docs/images/hero.png" alt="A photo next to its ASCII art version" width="860">
-->

</div>

## Features

- **Looks like the photo.** Characters are chosen by the *shape* of each cell, not just its brightness, so outlines, hair and edges follow `/ \ | _ ( )` strokes while flat areas stay smooth.
- **Calibrated to the font.** Every glyph's real ink coverage is measured in the bundled JetBrains Mono, so tones come out right instead of guessed from a hand-written ramp.
- **Animated SVG export.** Four reveal animations — typewriter, line by line, random, and growing from the centre — that play anywhere an image can, including a GitHub README, with no scripts.
- **PNG and plain text too.** Same art, three formats.
- **Any output size.** Defaults to your image's own size; one-click presets for avatars, posts, stories, HD, 4K and social previews, with 1–4× scaling.
- **Fast and private.** Multi-threaded native CLI; the web app runs the same Rust engine as WebAssembly, entirely on your device. Nothing is uploaded.
- **Quality you can measure.** Built-in SSIM scoring, golden tests and quality floors keep conversions from regressing.

<!-- IMAGE SLOT: docs/images/demo.svg — an animated reveal exported from the app.
<p align="center">
  <img src="docs/images/demo.svg" alt="Animated ASCII reveal" width="600">
</p>
-->

## Web app

**[kylehesi.github.io/image2ascii-studio](https://kylehesi.github.io/image2ascii-studio/)** — no installation.

Open it, drop, paste (<kbd>Ctrl</kbd>+<kbd>V</kbd>) or choose an image — or try the built-in sample — and tune it live: width, matching mode, character set, background, dithering and tone. Pick a reveal animation and preview it, then export TXT, SVG or PNG — or copy the text. Settings are remembered between visits.

> **Viewing the TXT in Notepad?** Turn off *View › Word wrap* — wide art has long lines, and wrapping them scrambles the picture.

<!-- IMAGE SLOT: docs/images/web-app.png — the web app with an image loaded.
<p align="center">
  <img src="docs/images/web-app.png" alt="The web app" width="860">
</p>
-->

## Command line

### Install

Download the archive for your platform from the [latest release](https://github.com/kylehesi/image2ascii-studio/releases/latest) (Windows, macOS Intel/Apple Silicon, Linux), unpack it and put `image2ascii` on your `PATH`.

Or build it with Rust:

```sh
cargo install --git https://github.com/kylehesi/image2ascii-studio image2ascii-studio
```

### Use

```sh
image2ascii photo.jpg                                   # print to the terminal
image2ascii photo.jpg --reveal                          # ... with the typewriter animation
image2ascii photo.jpg --reveal --reveal-order centre    # ... or lines, random, centre
image2ascii photo.jpg -o art.txt -o art.svg -o art.png  # save; format from the extension
image2ascii photo.jpg -c 160 -o art.svg                 # more detail (160 characters wide)
image2ascii photo.jpg --size 1080x1080 -o post.png      # exact image size, art centred
image2ascii photo.jpg --scale 2 -o art.png              # twice the original size
image2ascii photo.jpg --background light -o art.svg     # dark ink: light pages, or the "negative" look
```

| Option | What it does |
|---|---|
| `-c, --cols 120` | Width in characters — the level of detail |
| `-m, --mode shape\|density` | Match by tone pattern (edges) or average tone (smooth) |
| `--charset auto\|simple\|standard\|structure\|full`, `--chars "…"` | Built-in or custom character set |
| `--background dark\|light` | What the art is shown on |
| `--dither 0..1` | Error diffusion for smoother gradients |
| `--contrast`, `--brightness`, `--gamma`, `--clahe`, `--sharpen` | Tone adjustments |
| `--size original\|WxH`, `--scale` | SVG/PNG image size (default: the input's size) |
| `--no-frame`, `--title`, `--fg`, `--bg` | Window frame and colours |
| `--reveal-order typewriter\|lines\|random\|centre` | Reveal animation (terminal and SVG) |
| `--reveal-ms 3000`, `--loop-pause 2000`, `--no-animate` | Animation length, SVG looping, static SVG |
| `--svg-text` | Smaller SVG using `<text>` (drawn in the viewer's font) |
| `--score` | Print SSIM and tone error against the source |

Run `image2ascii --help` for everything.

### In a GitHub README

Export an SVG and reference it like any image — the reveal plays on GitHub:

```md
<img src="art.svg" width="600" alt="ASCII art">
```

## How it works

1. **Decode** — PNG, JPEG, WebP, GIF, BMP or TIFF; EXIF orientation, 16-bit and transparency handled; oversized images are reduced in linear light.
2. **Tone** — auto levels, local contrast (CLAHE), contrast, brightness and gamma, applied on perceptual lightness.
3. **Sample** — the image is averaged exactly over each character cell (and a 4×8 grid inside it), in linear light, on the font's real cell proportions.
4. **Calibrate** — every character is rasterised in the font and its ink coverage and ink pattern are measured.
5. **Match** — each cell gets the character that best fits both its tone and its pattern: `(1−w)·tone² + w·2σ²·(1−corr)`, so edges pick strokes that run the same way and flat areas are matched on tone alone. Optional Floyd–Steinberg dithering.
6. **Render** — text, SVG (glyph outlines from the font, reveal as an SMIL animation) or PNG (composited in linear light so thin strokes keep their calibrated tone).

The core is plain Rust with no I/O; the CLI runs it on all cores, and the web app runs the very same code as single-threaded WebAssembly in a Web Worker — so both produce identical output.

## Development

Requirements: [Rust](https://rustup.rs) (stable), and for the web app [Node.js](https://nodejs.org) 20+, the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
```

```sh
cargo test                                   # unit, golden snapshot and quality-floor tests
cargo run --release -p image2ascii-studio -- photo.jpg --reveal
cargo bench -p image2ascii-core

cd web
npm install
npm run dev                                  # builds the WebAssembly engine, serves http://localhost:5173
npm run build                                # static site in web/dist
```

After an intended change to conversion output, regenerate the snapshots with `UPDATE_GOLDEN=1 cargo test -p image2ascii-core --test golden` and review the diff.

| Path | Contents |
|---|---|
| `crates/core` | Conversion engine: decoding, tone, sampling, glyph calibration, matching, scoring, SVG/PNG rendering |
| `crates/cli` | The `image2ascii` command |
| `crates/wasm` | WebAssembly bridge for the web app |
| `web` | Web app (TypeScript, Vite) |
| `assets/fonts` | Bundled JetBrains Mono and its licence |

## License

[MIT](LICENSE) © kylehesi.

The bundled font, [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono), is © The JetBrains Mono Project Authors and licensed under the [SIL Open Font License 1.1](assets/fonts/JetBrainsMono-OFL.txt).
