// Runs the WebAssembly engine off the UI thread. Convert requests coalesce:
// while one runs, newer ones replace older queued ones, so dragging a slider
// never builds a backlog. The quality score is computed only when no newer
// request is waiting.

import init, { Engine } from './wasm/ascii_wasm.js';
import type { ConvertOptions, ExportOptions, Geometry, Request, Response } from './protocol';

/** Matches the core's working-size cap; larger images are resized while decoding. */
const MAX_SIDE = 4096;

const post = (msg: Response, transfer: Transferable[] = []) => self.postMessage(msg, { transfer });

let engine: Engine;
let queue: Request[] = [];
let running = false;

const ready = (async () => {
  await init();
  engine = new Engine();
  post({
    type: 'ready',
    defaults: Engine.defaults() as ConvertOptions,
    metrics: Array.from(engine.fontMetrics()) as [number, number, number, number],
  });
})();

self.onmessage = (e: MessageEvent<Request>) => {
  const req = e.data;
  if (req.type === 'convert') queue = queue.filter((r) => r.type !== 'convert');
  queue.push(req);
  void pump();
};

/** Lets queued `onmessage` events run before deciding what to do next. */
const yieldToMessages = () => new Promise((r) => setTimeout(r, 0));

async function pump() {
  if (running) return;
  running = true;
  await ready;
  while (queue.length) {
    const req = queue.shift()!;
    try {
      await handle(req);
    } catch (err) {
      post({ type: 'error', id: req.id, message: err instanceof Error ? err.message : String(err) });
    }
    await yieldToMessages();
  }
  running = false;
}

async function handle(req: Request) {
  switch (req.type) {
    case 'load': {
      const start = performance.now();
      await load(req.file);
      const [width, height] = engine.sourceSize() ?? [0, 0];
      const [originalWidth, originalHeight] = engine.originalSize() ?? [width, height];
      post({ type: 'loaded', id: req.id, width, height, originalWidth, originalHeight, ms: performance.now() - start });
      break;
    }
    case 'convert': {
      const start = performance.now();
      const result = engine.convert(req.options) as { text: string; cols: number; rows: number };
      const ms = performance.now() - start;
      post({ type: 'converted', id: req.id, ...result, ms, geometry: geometry(req.export) });
      await yieldToMessages();
      if (!queue.some((r) => r.type === 'convert')) {
        const score = engine.score() as { ssim: number; toneError: number };
        post({ type: 'score', id: req.id, ...score });
      }
      break;
    }
    case 'geometry': {
      post({ type: 'geometry', id: req.id, geometry: geometry(req.export) });
      break;
    }
    case 'export': {
      const options = engineExport(req.options).options;
      if (req.format === 'svg') {
        post({ type: 'exported', id: req.id, format: 'svg', data: engine.svg(options) });
      } else {
        const png = engine.png(options);
        post({ type: 'exported', id: req.id, format: 'png', data: png }, [png.buffer]);
      }
      break;
    }
    case 'schedule': {
      const times = engine.textSchedule(req.durationMs);
      post({ type: 'schedule', id: req.id, times }, [times.buffer]);
      break;
    }
  }
}

/**
 * Resolves the UI's size choice (original size or a preset, times the scale,
 * capped to the canvas limits) into the engine's exact width and height.
 */
function engineExport(opts: ExportOptions) {
  const [w, h] = opts.size ?? Array.from(engine.originalSize() ?? [512, 512]);
  const [width, height, capped] = Engine.fitCanvas(w, h, opts.scale);
  const options = {
    width,
    height,
    frame: opts.frame,
    title: opts.title,
    animate: opts.animate,
    revealMs: opts.revealMs,
    loopPauseMs: opts.loopPauseMs,
    svgText: opts.svgText,
  };
  return { options, capped: capped === 1 };
}

function geometry(opts: ExportOptions): Geometry {
  const { options, capped } = engineExport(opts);
  return { ...(engine.geometry(options) as Omit<Geometry, 'capped'>), capped };
}

/**
 * Decodes with the browser where possible (any format it supports, EXIF
 * orientation applied), falling back to the engine's own decoder.
 */
async function load(file: Blob) {
  let bitmap: ImageBitmap | undefined;
  let original: [number, number] | undefined;
  try {
    bitmap = await createImageBitmap(file);
    original = [bitmap.width, bitmap.height];
    const scale = Math.min(1, MAX_SIDE / Math.max(bitmap.width, bitmap.height));
    if (scale < 1) {
      const resized = await createImageBitmap(file, {
        resizeWidth: Math.max(1, Math.round(bitmap.width * scale)),
        resizeHeight: Math.max(1, Math.round(bitmap.height * scale)),
        resizeQuality: 'high',
      });
      bitmap.close();
      bitmap = resized;
    }
  } catch {
    bitmap = undefined;
  }
  if (!bitmap || typeof OffscreenCanvas === 'undefined') {
    bitmap?.close();
    engine.loadEncoded(new Uint8Array(await file.arrayBuffer()));
    return;
  }
  const { width, height } = bitmap;
  const ctx = new OffscreenCanvas(width, height).getContext('2d', { willReadFrequently: true });
  if (!ctx) throw new Error('2D canvas unavailable');
  ctx.drawImage(bitmap, 0, 0);
  bitmap.close();
  const pixels = ctx.getImageData(0, 0, width, height).data;
  engine.loadRgba(width, height, new Uint8Array(pixels.buffer, pixels.byteOffset, pixels.byteLength), original?.[0], original?.[1]);
}
