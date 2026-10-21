import './style.css';
import licenseUrl from '../../assets/fonts/JetBrainsMono-OFL.txt?url';
import type { ConvertOptions, ExportOptions, Geometry, Request, Response } from './protocol';

// ---------------------------------------------------------------- worker

const worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
let nextId = 1;
const waiting = new Map<number, (r: Response) => void>();

type Outgoing = Request extends infer R ? (R extends { id: number } ? Omit<R, 'id'> : never) : never;

function send(req: Outgoing): number {
  const id = nextId++;
  worker.postMessage({ ...req, id } as Request);
  return id;
}

/** Sends a request and resolves with its response (or rejects with its error). */
function call(req: Outgoing): Promise<Response> {
  return new Promise((resolve, reject) => {
    const id = send(req);
    waiting.set(id, (r) => (r.type === 'error' ? reject(new Error(r.message)) : resolve(r)));
  });
}

// ---------------------------------------------------------------- output sizes

type SizePreset = { id: string; size: [number, number]; note: string };

/** Popular canvas sizes. The art is fitted inside and centred. */
const SIZE_PRESETS: SizePreset[] = [
  { id: '512x512', size: [512, 512], note: 'Avatar' },
  { id: '1080x1080', size: [1080, 1080], note: 'Square post' },
  { id: '1080x1350', size: [1080, 1350], note: 'Portrait 4:5' },
  { id: '1080x1920', size: [1080, 1920], note: 'Story 9:16' },
  { id: '1280x720', size: [1280, 720], note: 'HD' },
  { id: '1920x1080', size: [1920, 1080], note: 'Full HD' },
  { id: '3840x2160', size: [3840, 2160], note: '4K' },
  { id: '1280x640', size: [1280, 640], note: 'GitHub social' },
  { id: '1200x630', size: [1200, 630], note: 'Link preview' },
];

// ---------------------------------------------------------------- state

type ExportState = {
  /** 'original', 'custom' or a SIZE_PRESETS id. */
  sizePreset: string;
  scale: number;
  customWidth: number;
  customHeight: number;
  frame: boolean;
  title: string;
  animate: boolean;
  revealMs: number;
  loop: boolean;
  loopPause: number;
  svgText: boolean;
};

type UiState = {
  options: ConvertOptions;
  charsetChoice: ConvertOptions['charset'] | 'custom';
  customChars: string;
  export: ExportState;
};

const STORAGE_KEY = 'image2ascii-studio:v1';

let defaults: ConvertOptions;
/** Per pixel of font size: advance, line height, ascent, descent (negative). */
let metrics: [number, number, number, number] = [0.6, 1.32, 1.02, -0.3];
let state: UiState;
let sourceName = 'ascii';
let hasImage = false;
let original: [number, number] | undefined;
let result: { text: string; cols: number; rows: number; ms: number } | undefined;
let geometry: Geometry | undefined;
let score: { ssim: number; toneError: number } | undefined;
let latestConvert = 0;
let latestGeometry = 0;
let actualSize = false;
let playing: number | undefined;

const exportDefaults: ExportState = {
  sizePreset: 'original',
  scale: 1,
  customWidth: 1000,
  customHeight: 1000,
  frame: true,
  title: '',
  animate: true,
  revealMs: 3000,
  loop: false,
  loopPause: 2000,
  svgText: false,
};

function initialState(): UiState {
  const fresh: UiState = {
    options: { ...defaults },
    charsetChoice: 'auto',
    customChars: ' .:-=+*#%@',
    export: { ...exportDefaults },
  };
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null') as Partial<UiState> | null;
    if (saved) {
      return {
        options: { ...fresh.options, ...saved.options },
        charsetChoice: saved.charsetChoice ?? fresh.charsetChoice,
        customChars: saved.customChars ?? fresh.customChars,
        export: { ...fresh.export, ...saved.export },
      };
    }
  } catch {
    // Storage unavailable or corrupt: start fresh.
  }
  return fresh;
}

function save() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Private mode etc. — settings just won't persist.
  }
}

// ---------------------------------------------------------------- dom

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const art = $<HTMLPreElement>('art');
const sizer = $<HTMLDivElement>('sizer');
const canvasEl = $<HTMLDivElement>('canvas');
const titleEl = $<HTMLSpanElement>('title');
const dots = Array.from(canvasEl.querySelectorAll<HTMLSpanElement>('.dot'));
const viewport = $<HTMLDivElement>('viewport');
const stats = $<HTMLSpanElement>('stats');
const toastEl = $<HTMLDivElement>('toast');
const buttons = ['play', 'zoom', 'copy', 'dl-txt', 'dl-svg', 'dl-png'].map((id) => $<HTMLButtonElement>(id));
$<HTMLAnchorElement>('license').href = licenseUrl;

let toastTimer: number | undefined;
function toast(message: string, error = false) {
  toastEl.textContent = message;
  toastEl.classList.toggle('error', error);
  toastEl.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toastEl.hidden = true), error ? 6000 : 2500);
}

// ---------------------------------------------------------------- controls

/** Re-syncs every control with `state` (after reset). */
const refreshers: (() => void)[] = [];

function field(parent: HTMLElement, label: string, control: HTMLElement, output?: HTMLOutputElement) {
  const wrap = document.createElement('div');
  wrap.className = 'field';
  const head = document.createElement('label');
  head.textContent = label;
  if (output) head.append(output);
  wrap.append(head, control);
  parent.append(wrap);
  return wrap;
}

function range(
  parent: HTMLElement,
  label: string,
  spec: { min: number; max: number; step: number; format?: (v: number) => string },
  get: () => number,
  set: (v: number) => void,
) {
  const input = document.createElement('input');
  input.type = 'range';
  Object.assign(input, { min: String(spec.min), max: String(spec.max), step: String(spec.step) });
  input.setAttribute('aria-label', label);
  const out = document.createElement('output');
  const format = spec.format ?? ((v: number) => String(v));
  const refresh = () => {
    input.value = String(get());
    out.textContent = format(get());
  };
  input.addEventListener('input', () => {
    set(Number(input.value));
    out.textContent = format(Number(input.value));
  });
  refresh();
  refreshers.push(refresh);
  return field(parent, label, input, out);
}

function choice<T extends string | number>(
  parent: HTMLElement,
  label: string,
  options: [T, string][],
  get: () => T,
  set: (v: T) => void,
) {
  const group = document.createElement('div');
  group.className = 'segmented';
  group.setAttribute('role', 'group');
  group.setAttribute('aria-label', label);
  const btns = options.map(([value, text]) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = text;
    b.addEventListener('click', () => {
      set(value);
      refresh();
    });
    group.append(b);
    return [value, b] as const;
  });
  const refresh = () => btns.forEach(([v, b]) => b.setAttribute('aria-pressed', String(v === get())));
  refresh();
  refreshers.push(refresh);
  const wrap = document.createElement('div');
  wrap.className = 'field';
  const head = document.createElement('span');
  head.className = 'label';
  head.textContent = label;
  wrap.append(head, group);
  parent.append(wrap);
  return wrap;
}

function check(parent: HTMLElement, label: string, get: () => boolean, set: (v: boolean) => void) {
  const wrap = document.createElement('label');
  wrap.className = 'check';
  const input = document.createElement('input');
  input.type = 'checkbox';
  input.addEventListener('change', () => set(input.checked));
  const refresh = () => (input.checked = get());
  refresh();
  refreshers.push(refresh);
  wrap.append(input, label);
  parent.append(wrap);
  return wrap;
}

function text(parent: HTMLElement, label: string, placeholder: string, get: () => string, set: (v: string) => void) {
  const input = document.createElement('input');
  input.type = 'text';
  input.placeholder = placeholder;
  input.spellcheck = false;
  input.addEventListener('input', () => set(input.value));
  const refresh = () => (input.value = get());
  refresh();
  refreshers.push(refresh);
  return field(parent, label, input);
}

/** Preset chips: one press picks the canvas size. */
function sizeChips(parent: HTMLElement, set: (id: string) => void) {
  const grid = document.createElement('div');
  grid.className = 'chips';
  grid.setAttribute('role', 'group');
  grid.setAttribute('aria-label', 'Output size');
  const entries: [string, string, string][] = [
    ['original', 'Original', ''],
    ...SIZE_PRESETS.map((p): [string, string, string] => [p.id, `${p.size[0]} × ${p.size[1]}`, p.note]),
    ['custom', 'Custom', 'Any size'],
  ];
  const chips = entries.map(([id, label, note]) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'chip';
    const strong = document.createElement('strong');
    strong.textContent = label;
    const small = document.createElement('small');
    small.textContent = note;
    b.append(strong, small);
    b.addEventListener('click', () => {
      set(id);
      refresh();
    });
    grid.append(b);
    return [id, b, small] as const;
  });
  const refresh = () => {
    for (const [id, b, small] of chips) {
      b.setAttribute('aria-pressed', String(id === state.export.sizePreset));
      if (id === 'original') small.textContent = original ? `${original[0]} × ${original[1]}` : 'Image size';
    }
  };
  refresh();
  refreshers.push(refresh);
  const wrap = document.createElement('div');
  wrap.className = 'field';
  const head = document.createElement('span');
  head.className = 'label';
  head.textContent = 'Output size (SVG/PNG)';
  wrap.append(head, grid);
  parent.append(wrap);
}

function customSize(parent: HTMLElement, onChange: () => void) {
  const row = document.createElement('div');
  row.className = 'size-row';
  const input = (label: string, get: () => number, set: (v: number) => void) => {
    const el = document.createElement('input');
    el.type = 'number';
    Object.assign(el, { min: '32', max: '8192', step: '1' });
    el.setAttribute('aria-label', label);
    el.addEventListener('change', () => {
      const v = Math.round(Number(el.value));
      if (Number.isFinite(v) && v >= 32 && v <= 8192) {
        set(v);
        onChange();
      } else {
        el.value = String(get());
        toast('Sizes must be between 32 and 8192 px.', true);
      }
    });
    refreshers.push(() => (el.value = String(get())));
    el.value = String(get());
    return el;
  };
  const times = document.createElement('span');
  times.textContent = '×';
  row.append(
    input('Custom width', () => state.export.customWidth, (v) => (state.export.customWidth = v)),
    times,
    input('Custom height', () => state.export.customHeight, (v) => (state.export.customHeight = v)),
  );
  return field(parent, 'Custom size (px)', row);
}

/** Shows a control only while `when` holds. */
const conditional: [HTMLElement, () => boolean][] = [];
function showWhen(el: HTMLElement, when: () => boolean) {
  conditional.push([el, when]);
}
function updateVisibility() {
  for (const [el, when] of conditional) el.hidden = !when();
}

function buildControls() {
  const conv = $('convert-controls');
  const tone = $('tone-controls');
  const exp = $('export-controls');
  const o = () => state.options;
  const changed = () => {
    updateVisibility();
    save();
    requestConvert();
  };

  range(conv, 'Width (characters)', { min: 20, max: 400, step: 1 }, () => o().cols, (v) => {
    o().cols = v;
    changed();
  });
  choice(conv, 'Matching', [['shape', 'Shape'], ['density', 'Density']], () => o().mode, (v) => {
    o().mode = v;
    changed();
  });
  showWhen(
    range(conv, 'Shape weight', { min: 0, max: 1, step: 0.05, format: (v) => v.toFixed(2) }, () => o().shapeWeight, (v) => {
      o().shapeWeight = v;
      changed();
    }),
    () => o().mode === 'shape',
  );

  const select = document.createElement('select');
  select.setAttribute('aria-label', 'Character set');
  for (const [value, label] of [
    ['auto', 'Auto (by mode)'],
    ['standard', 'Standard — smooth tones'],
    ['structure', 'Structure — edges with / \\ | _'],
    ['simple', 'Simple — 10 characters'],
    ['full', 'Full — all printable ASCII'],
    ['custom', 'Custom…'],
  ]) {
    select.add(new Option(label, value));
  }
  select.addEventListener('change', () => {
    state.charsetChoice = select.value as UiState['charsetChoice'];
    changed();
  });
  refreshers.push(() => (select.value = state.charsetChoice));
  select.value = state.charsetChoice;
  field(conv, 'Character set', select);
  showWhen(
    text(conv, 'Custom characters', 'e.g.  .:-=+*#%@', () => state.customChars, (v) => {
      state.customChars = v;
      changed();
    }),
    () => state.charsetChoice === 'custom',
  );

  choice(conv, 'Background', [['dark', 'Dark'], ['light', 'Light / negative']], () => o().background, (v) => {
    o().background = v;
    changed();
  });
  range(conv, 'Dithering', { min: 0, max: 1, step: 0.05, format: (v) => v.toFixed(2) }, () => o().dither, (v) => {
    o().dither = v;
    changed();
  });

  check(tone, 'Auto levels (tone stretch)', () => o().autoLevels, (v) => {
    o().autoLevels = v;
    changed();
  });
  const signed = (v: number) => (v > 0 ? '+' : '') + v.toFixed(2);
  range(tone, 'Contrast', { min: 0, max: 3, step: 0.05, format: (v) => v.toFixed(2) }, () => o().contrast, (v) => {
    o().contrast = v;
    changed();
  });
  range(tone, 'Brightness', { min: -0.5, max: 0.5, step: 0.01, format: signed }, () => o().brightness, (v) => {
    o().brightness = v;
    changed();
  });
  range(tone, 'Gamma', { min: 0.3, max: 3, step: 0.05, format: (v) => v.toFixed(2) }, () => o().gamma, (v) => {
    o().gamma = v;
    changed();
  });
  range(tone, 'Local contrast', { min: 0, max: 1, step: 0.05, format: (v) => v.toFixed(2) }, () => o().clahe, (v) => {
    o().clahe = v;
    changed();
  });
  range(tone, 'Sharpen', { min: 0, max: 3, step: 0.05, format: (v) => v.toFixed(2) }, () => o().sharpen, (v) => {
    o().sharpen = v;
    changed();
  });
  choice(tone, 'Tone mapping', [['perceptual', 'Perceptual'], ['linear', 'Linear']], () => o().tone, (v) => {
    o().tone = v;
    changed();
  });

  const e = () => state.export;
  const exportChanged = () => {
    updateVisibility();
    save();
    requestGeometry();
  };
  sizeChips(exp, (id) => {
    e().sizePreset = id;
    exportChanged();
  });
  showWhen(customSize(exp, exportChanged), () => e().sizePreset === 'custom');
  choice(exp, 'Scale', [[1, '1×'], [2, '2×'], [3, '3×'], [4, '4×']], () => e().scale, (v) => {
    e().scale = v;
    exportChanged();
  });
  const info = document.createElement('p');
  info.className = 'muted size-info';
  info.id = 'size-info';
  exp.append(info);

  check(exp, 'Window frame', () => e().frame, (v) => {
    e().frame = v;
    exportChanged();
  });
  showWhen(
    text(exp, 'Title', 'image-name.txt', () => e().title, (v) => {
      e().title = v;
      exportChanged();
    }),
    () => e().frame,
  );
  check(exp, 'SVG animation', () => e().animate, (v) => {
    e().animate = v;
    exportChanged();
  });
  showWhen(
    range(exp, 'Animation length', { min: 500, max: 10000, step: 100, format: (v) => `${(v / 1000).toFixed(1)} s` }, () => e().revealMs, (v) => {
      e().revealMs = v;
      exportChanged();
    }),
    () => e().animate,
  );
  showWhen(
    check(exp, 'Loop (replay when finished)', () => e().loop, (v) => {
      e().loop = v;
      exportChanged();
    }),
    () => e().animate,
  );
  showWhen(
    range(exp, 'Loop pause', { min: 0, max: 10000, step: 250, format: (v) => `${(v / 1000).toFixed(2)} s` }, () => e().loopPause, (v) => {
      e().loopPause = v;
      exportChanged();
    }),
    () => e().animate && e().loop,
  );
  check(exp, "SVG text mode (smaller file, viewer's font)", () => e().svgText, (v) => {
    e().svgText = v;
    exportChanged();
  });

  $('reset').addEventListener('click', () => {
    state = { ...state, options: { ...defaults }, charsetChoice: 'auto' };
    refreshers.forEach((r) => r());
    changed();
  });
  updateVisibility();
}

// ---------------------------------------------------------------- conversion

function convertOptions(): ConvertOptions {
  const o = { ...state.options };
  if (state.charsetChoice === 'custom') {
    o.charset = 'auto';
    o.chars = state.customChars;
  } else {
    o.charset = state.charsetChoice;
    delete o.chars;
  }
  return o;
}

let convertQueued = false;
/**
 * Coalesces changes made in the same task (e.g. a reset touching every
 * option) into one request. Across events the worker does the coalescing: a
 * queued request is replaced by a newer one. A microtask, not
 * requestAnimationFrame, so a tab in the background still converts.
 */
function requestConvert() {
  if (!hasImage || convertQueued) return;
  convertQueued = true;
  queueMicrotask(() => {
    convertQueued = false;
    latestConvert = send({ type: 'convert', options: convertOptions(), export: exportOptions() });
  });
}

/** Re-lays out the current result after an export option changed. */
function requestGeometry() {
  if (!result) return;
  latestGeometry = send({ type: 'geometry', export: exportOptions() });
}

function exportOptions(): ExportOptions {
  const e = state.export;
  const preset = SIZE_PRESETS.find((p) => p.id === e.sizePreset);
  const size: [number, number] | null =
    e.sizePreset === 'custom' ? [e.customWidth, e.customHeight] : preset ? preset.size : null;
  return {
    size,
    scale: e.scale,
    frame: e.frame,
    title: titleText(),
    animate: e.animate,
    revealMs: e.revealMs,
    loopPauseMs: e.animate && e.loop ? e.loopPause : undefined,
    svgText: e.svgText,
  };
}

const titleText = () => state.export.title.trim() || `${sourceName}.txt`;

function showResult() {
  if (!result || !geometry) return;
  stopPlaying();
  art.textContent = result.text;
  sizer.hidden = false;
  $('empty').hidden = true;
  buttons.forEach((b) => (b.disabled = false));
  applyGeometry();
  renderStats();
}

function renderStats() {
  if (!result) return;
  const parts = [`${result.cols}×${result.rows} characters`, `${result.ms.toFixed(0)} ms`];
  if (score) parts.push(`SSIM ${score.ssim.toFixed(3)}`, `tone error ${score.toneError.toFixed(3)}`);
  stats.textContent = parts.join(' · ');
}

/**
 * Draws the preview exactly like the export: a canvas of the output size in
 * canvas pixels (frame, dots, title, art at the computed font size), then
 * scaled as a whole to fit the viewport.
 */
function applyGeometry() {
  const g = geometry;
  if (!g) return;
  const [, line, ascent, descent] = metrics;
  // Where CSS puts the baseline in a line box of height `line·px`; matches
  // the renderer's centring of the ascent..descent box.
  const baselineOffset = (px: number) => (line * px - (ascent - descent) * px) / 2 + ascent * px;

  Object.assign(canvasEl.style, {
    width: `${g.width}px`,
    height: `${g.height}px`,
    background: g.bg,
    color: g.fg,
    borderRadius: g.frame ? `${g.frame.radius}px` : '0',
    boxShadow: g.frame ? `inset 0 0 0 1px ${g.frame.border}` : 'none',
  });
  Object.assign(art.style, {
    left: `${g.originX}px`,
    top: `${g.originY}px`,
    fontSize: `${g.fontPx}px`,
    lineHeight: `${g.cellH}px`,
  });
  const f = g.frame;
  titleEl.hidden = !f;
  dots.forEach((d, i) => {
    d.hidden = !f;
    if (!f) return;
    Object.assign(d.style, {
      left: `${f.dots[i][0] - f.dotRadius}px`,
      top: `${f.dots[i][1] - f.dotRadius}px`,
      width: `${f.dotRadius * 2}px`,
      height: `${f.dotRadius * 2}px`,
      background: f.dotColors[i],
    });
  });
  if (f) {
    titleEl.textContent = titleText();
    Object.assign(titleEl.style, {
      left: `${f.titleCentreX}px`,
      top: `${f.titleBaseline - baselineOffset(f.titlePx)}px`,
      fontSize: `${f.titlePx}px`,
      lineHeight: `${line * f.titlePx}px`,
      color: f.titleColor,
    });
  }
  const info = document.getElementById('size-info');
  if (info) {
    info.textContent =
      `Output ${Math.round(g.width)} × ${Math.round(g.height)} px · glyphs ${g.fontPx.toFixed(1)} px` +
      (g.capped ? ' · reduced to the size limit' : '');
  }
  fitToViewport();
}

/** Scales the canvas to fit the viewport, or shows it 1:1 in actual-size mode. */
function fitToViewport() {
  const g = geometry;
  if (!g) return;
  const style = getComputedStyle(viewport);
  const availW = viewport.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  const availH = viewport.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom);
  const k = actualSize ? 1 : Math.max(0.05, Math.min(availW / g.width, availH / g.height));
  canvasEl.style.transform = `scale(${k})`;
  sizer.style.width = `${g.width * k}px`;
  sizer.style.height = `${g.height * k}px`;
  $('zoom').textContent = actualSize ? 'Fit' : 'Actual size';
}

new ResizeObserver(() => fitToViewport()).observe(viewport);

// ---------------------------------------------------------------- reveal preview

function stopPlaying() {
  if (playing !== undefined) cancelAnimationFrame(playing);
  playing = undefined;
  $('play').textContent = '▶ Play';
}

async function play() {
  if (!result) return;
  if (playing !== undefined) {
    stopPlaying();
    art.textContent = result.text;
    return;
  }
  const shown = result;
  const r = await call({ type: 'schedule', durationMs: state.export.revealMs });
  if (r.type !== 'schedule' || result !== shown) return;
  const chars = Array.from(shown.text);
  const times = r.times;
  const start = performance.now();
  $('play').textContent = '■ Stop';
  const frame = () => {
    const t = performance.now() - start;
    // First index whose time is still in the future.
    let lo = 0;
    let hi = times.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (times[mid] <= t) lo = mid + 1;
      else hi = mid;
    }
    art.textContent = chars.slice(0, lo).join('');
    if (lo < chars.length) playing = requestAnimationFrame(frame);
    else stopPlaying();
  };
  playing = requestAnimationFrame(frame);
}

// ---------------------------------------------------------------- input

async function loadFile(file: Blob, name: string) {
  sourceName = name.replace(/\.[^.]+$/, '') || 'ascii';
  setLandingBusy(true);
  try {
    const r = await call({ type: 'load', file });
    if (r.type !== 'loaded') return;
    hasImage = true;
    original = [r.originalWidth, r.originalHeight];
    refreshers.forEach((f) => f());
    $('source-info').textContent = `${name} — ${r.originalWidth}×${r.originalHeight}`;
    requestConvert();
  } catch (err) {
    setLandingBusy(false);
    toast(`Couldn't open the image: ${(err as Error).message}`, true);
  }
}

function imageFrom(list: DataTransferItemList | FileList | null | undefined): File | undefined {
  if (!list) return undefined;
  for (const item of Array.from(list as ArrayLike<DataTransferItem | File>)) {
    const file = item instanceof File ? item : item.kind === 'file' ? item.getAsFile() : null;
    if (file && (file.type.startsWith('image/') || /\.(png|jpe?g|webp|gif|bmp|tiff?|avif|heic)$/i.test(file.name))) {
      return file;
    }
  }
  return undefined;
}

/** A file input inside a focusable drop label: click, Enter/Space or choose. */
function wirePicker(inputId: string, dropId: string) {
  const input = $<HTMLInputElement>(inputId);
  input.addEventListener('change', () => {
    const file = input.files?.[0];
    if (file) void loadFile(file, file.name);
    input.value = '';
  });
  $(dropId).addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      input.click();
    }
  });
}

function wireInput() {
  wirePicker('landing-file', 'landing-drop');
  wirePicker('file', 'drop');
  let depth = 0;
  window.addEventListener('dragenter', (e) => {
    e.preventDefault();
    depth++;
    document.body.classList.add('dragging');
  });
  window.addEventListener('dragleave', () => {
    if (--depth <= 0) {
      depth = 0;
      document.body.classList.remove('dragging');
    }
  });
  window.addEventListener('dragover', (e) => e.preventDefault());
  window.addEventListener('drop', (e) => {
    e.preventDefault();
    depth = 0;
    document.body.classList.remove('dragging');
    const file = imageFrom(e.dataTransfer?.files);
    if (file) void loadFile(file, file.name);
    else toast("That drop wasn't an image.", true);
  });
  window.addEventListener('paste', (e) => {
    const file = imageFrom(e.clipboardData?.items);
    if (file) void loadFile(file, file.name || 'pasted.png');
  });
  $('landing-sample').addEventListener('click', async () => {
    const blob = await sampleImage();
    void loadFile(blob, 'sample.png');
  });
}

/** Shows progress on the landing screen while the first image opens. */
function setLandingBusy(busy: boolean) {
  $('landing-drop').classList.toggle('busy', busy);
  $('landing-label').textContent = busy ? 'Opening…' : 'Drop an image here';
}

/** A procedurally drawn test scene: smooth shading, hard edges, fine lines. */
async function sampleImage(): Promise<Blob> {
  const w = 640;
  const h = 480;
  const canvas = document.createElement('canvas');
  canvas.width = w;
  canvas.height = h;
  const g = canvas.getContext('2d')!;
  const sky = g.createLinearGradient(0, 0, 0, h);
  sky.addColorStop(0, '#0b1020');
  sky.addColorStop(1, '#5a6b8a');
  g.fillStyle = sky;
  g.fillRect(0, 0, w, h);
  for (let i = 0; i < 14; i++) {
    g.strokeStyle = `rgba(255,255,255,${0.15 + i * 0.03})`;
    g.lineWidth = 2;
    g.beginPath();
    g.moveTo(0, 330 + i * 11);
    g.lineTo(w, 300 + i * 13);
    g.stroke();
  }
  const sphere = g.createRadialGradient(250, 170, 10, 300, 220, 150);
  sphere.addColorStop(0, '#ffffff');
  sphere.addColorStop(0.45, '#9aa8c0');
  sphere.addColorStop(1, '#10131a');
  g.fillStyle = sphere;
  g.beginPath();
  g.arc(300, 220, 140, 0, Math.PI * 2);
  g.fill();
  g.strokeStyle = '#f0f3f8';
  g.lineWidth = 6;
  g.beginPath();
  g.ellipse(300, 230, 230, 52, -0.18, 0, Math.PI * 2);
  g.stroke();
  g.fillStyle = '#ffffff';
  g.font = 'bold 64px system-ui, sans-serif';
  g.fillText('ASCII', 420, 440);
  return new Promise((resolve, reject) => canvas.toBlob((b) => (b ? resolve(b) : reject(new Error('toBlob'))), 'image/png'));
}

// ---------------------------------------------------------------- output

function download(data: BlobPart, name: string, type: string) {
  const url = URL.createObjectURL(new Blob([data], { type }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

async function exportAs(format: 'svg' | 'png') {
  try {
    const r = await call({ type: 'export', format, options: exportOptions() });
    if (r.type !== 'exported') return;
    if (format === 'svg') download(r.data as string, `${sourceName}.svg`, 'image/svg+xml');
    else download(r.data as Uint8Array<ArrayBuffer>, `${sourceName}.png`, 'image/png');
  } catch (err) {
    toast(`Export failed: ${(err as Error).message}`, true);
  }
}

function wireActions() {
  $('play').addEventListener('click', () => void play());
  $('zoom').addEventListener('click', () => {
    actualSize = !actualSize;
    fitToViewport();
  });
  $('copy').addEventListener('click', async () => {
    if (!result) return;
    try {
      await navigator.clipboard.writeText(result.text);
      toast('Copied to clipboard');
    } catch {
      toast('Clipboard unavailable; select the text and copy it manually.', true);
    }
  });
  $('dl-txt').addEventListener('click', () => result && download(result.text, `${sourceName}.txt`, 'text/plain;charset=utf-8'));
  $('dl-svg').addEventListener('click', () => void exportAs('svg'));
  $('dl-png').addEventListener('click', () => void exportAs('png'));
}

// ---------------------------------------------------------------- messages

worker.onmessage = (e: MessageEvent<Response>) => {
  const msg = e.data;
  if (msg.type === 'ready') {
    defaults = msg.defaults;
    metrics = msg.metrics;
    state = initialState();
    buildControls();
    return;
  }
  const resolve = waiting.get(msg.id);
  if (resolve) {
    waiting.delete(msg.id);
    resolve(msg);
    return;
  }
  if (msg.type === 'geometry') {
    if (msg.id !== latestGeometry) return;
    geometry = msg.geometry;
    applyGeometry();
    return;
  }
  // Conversion results: only the newest request counts.
  if (msg.id !== latestConvert && msg.id !== latestGeometry) return;
  if (msg.type === 'converted') {
    result = msg;
    geometry = msg.geometry;
    score = undefined;
    // Leave the landing screen first so the preview measures a visible viewport.
    document.body.classList.add('has-image');
    setLandingBusy(false);
    showResult();
  } else if (msg.type === 'score') {
    score = msg;
    renderStats();
  } else if (msg.type === 'error') {
    toast(msg.message, true);
  }
};

worker.onerror = (e) => toast(`Couldn't load the engine: ${e.message}`, true);

wireInput();
wireActions();
