# img2ascii

Quality-first image → ASCII converter. One Rust core, used by a CLI and — compiled to WebAssembly — by a web app with live preview. Both produce identical output: the font (JetBrains Mono) is bundled.

## Quick test (Windows)

- **Web app:** double-click `web.bat`. First run installs the wasm tooling and npm packages; then the app opens in your browser. Everything runs locally — images never leave the browser.
- **CLI:** drag an image onto `img2ascii.bat`, or double-click it and type the path. It builds the CLI, asks for width / mode / polarity, plays the reveal animation and saves `<image>.txt`, `.svg` (animated) and `.png` next to the image.

## Web app (`web/`, `crates/wasm`)

Drop, paste (Ctrl+V) or pick an image; every slider re-converts live. Play previews the reveal animation; TXT / SVG / PNG / copy export the current result. Settings persist in the browser.

- `crates/wasm`: `Engine` (wasm-bindgen) — load once, convert many times. The tone-mapped image is cached (`ascii_core::Prepared`) while only grid/matching options change, so width, mode, charset and dither edits skip the full-resolution work.
- `web/src/worker.ts`: runs the engine in a Web Worker; browser-side decoding (EXIF-aware, any format the browser reads) with the engine's decoder as fallback; queued conversions coalesce so slider drags never back up; the SSIM score is computed only when idle.
- `web/src/main.ts`: vanilla TypeScript UI; the preview `<pre>` uses the bundled font with ligatures off, sized from the font's cell metrics so it shows exactly the calibrated grid.

```sh
cd web
npm install
npm run dev     # builds the wasm engine, then serves http://localhost:5173
npm run build   # production build in web/dist (static files, host anywhere)
```

Needs the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129 (must match the crate version; `web.bat` installs both).

## Usage

```sh
cargo run --release -p img2ascii -- photo.jpg -c 120 -o photo.txt
cargo run --release -p img2ascii -- photo.jpg -o art.txt -o art.svg -o art.png   # format from the extension
cargo run --release -p img2ascii -- photo.jpg -c 120 --reveal        # typewriter animation in the terminal
cargo run --release -p img2ascii -- photo.jpg -m density             # smooth tones instead of edge-following shapes
cargo run --release -p img2ascii -- photo.jpg --background light     # dark ink: for light pages, or the "negative" look
cargo run --release -p img2ascii -- photo.jpg --score                # print SSIM / tone error vs. the source
cargo run --release -p img2ascii -- --help
```

### SVG and PNG

Both draw the art in a window frame (dots + title, default `<input>.txt`) with GitHub's colours for the chosen background. The SVG plays the same typewriter reveal as `--reveal` and works as an `<img>` in a GitHub README (no scripts or external files):

```md
<img src="art.svg" width="600">
```

| Flag | Effect |
|---|---|
| `--font-size 16` | glyph size in px |
| `--no-frame`, `--title "me.txt"` | window chrome |
| `--fg "#c9d1d9" --bg "#0d1117"` | colours |
| `--reveal-ms 3000` | reveal length (terminal and SVG) |
| `--loop-pause 2000` | SVG: replay forever, holding the finished art 2 s |
| `--no-animate` | SVG: static |
| `--svg-text` | SVG: `<text>` rows (≈4× smaller, selectable) instead of exact glyph outlines; drawn in the viewer's font |

PNG compositing is done in linear light, so anti-aliased strokes keep the calibrated tones.

Glyph density is calibrated from a real monospace font: the bundled JetBrains Mono by default, or `--font path.ttf`. Plain `.txt` output looks best in the same font; SVG and PNG always use it.

## Font licence

`assets/fonts/JetBrainsMono-Regular.ttf` is JetBrains Mono, © 2020 The JetBrains Mono Project Authors, under the SIL Open Font License 1.1 (`assets/fonts/JetBrainsMono-OFL.txt`). The licence file must accompany any distribution of the font; the web build ships it and links to it from the page footer.

## Pipeline (`crates/core`)

1. **Decode** (`source.rs`): PNG/JPEG/WebP/GIF/BMP/TIFF, EXIF orientation, 8/16-bit, alpha; decoder size and allocation limits; inputs over 4096 px are box-filtered in linear light.
2. **Tone** (`preprocess.rs`): on CIE lightness: auto-levels, CLAHE local contrast, contrast/brightness/gamma; then back to linear light, with alpha composited onto the background.
3. **Resample** (`grid.rs`): rows from the font's cell aspect; exact area averaging (fractional pixel footprints) in linear light — one sample per cell (density) or 4×8 per cell (shape).
4. **Calibrate** (`atlas.rs`): every glyph rasterised; ink coverage and 4×8 ink pattern measured.
5. **Match** (`matcher.rs`):
   - *density*: nearest coverage per cell;
   - *shape* (default): `(1−w)·tone² + w·2σ²·(1−corr)` — tone error plus pattern mismatch scaled to the cell's own contrast, so edges get `/ \ | _ ( )` and flat areas are matched on tone;
   - optional Floyd–Steinberg error diffusion (`--dither`).
6. **Score** (`quality.rs`): art re-rendered on an 8×16 sub-cell grid, compared with SSIM and blurred tone error.
7. **Output**: `AsciiArt::to_text`; `render::render_svg` / `render::render_png` (`render/`), laid out on the same font cell metrics as calibration; `RevealTimeline` drives the terminal and SVG animations.

Multi-core via rayon (`parallel` feature, on by default; off for WebAssembly). Every parallel stage writes disjoint output, so results are bit-identical with or without it — the golden tests check both builds.

## Tests and benchmarks

```sh
cargo test                                  # unit + golden snapshot + quality-floor tests
cargo test -p ascii-core --no-default-features
UPDATE_GOLDEN=1 cargo test -p ascii-core --test golden   # after an intended output change; review the diff
cargo bench -p ascii-core
```

Golden tests use glyphs drawn in code and synthetic images (`crates/core/tests/common`), so they need no font or photo files.

## Dev tools

```sh
cargo run --release -p ascii-core --example preview -- <image> <font> out.png [cols=120 mode=shape charset=structure dither=0 weight=0.5 bg=dark]
cargo run --release -p ascii-core --example quality -- <font> <cols> <image>...   # sweep settings, mean SSIM / tone error
cargo run --release -p ascii-core --example glyphs -- <font>                      # per-glyph coverage / centroid table
```
