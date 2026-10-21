// Messages between the UI thread and the conversion worker.

export type ConvertOptions = {
  cols: number;
  mode: 'shape' | 'density';
  charset: 'auto' | 'simple' | 'standard' | 'structure' | 'full';
  chars?: string;
  background: 'dark' | 'light';
  tone: 'perceptual' | 'linear';
  shapeWeight: number;
  dither: number;
  autoLevels: boolean;
  clahe: number;
  contrast: number;
  brightness: number;
  gamma: number;
  sharpen: number;
};

export type ExportOptions = {
  /** Canvas size in pixels; `null` uses the loaded image's original size. */
  size: [number, number] | null;
  /** Multiplies `size`; the worker caps the result to the canvas limits. */
  scale: number;
  frame: boolean;
  title: string;
  animate: boolean;
  revealMs: number;
  loopPauseMs?: number;
  svgText: boolean;
};

/** Export layout in canvas pixels, straight from the Rust renderer. */
export type Geometry = {
  width: number;
  height: number;
  fontPx: number;
  originX: number;
  originY: number;
  cellW: number;
  cellH: number;
  fg: string;
  bg: string;
  frame?: {
    barHeight: number;
    radius: number;
    dotRadius: number;
    dots: [number, number][];
    dotColors: string[];
    titlePx: number;
    titleBaseline: number;
    titleCentreX: number;
    border: string;
    titleColor: string;
  };
  /** The requested size was larger than the canvas limits and was shrunk. */
  capped: boolean;
};

export type Request =
  | { type: 'load'; id: number; file: Blob }
  | { type: 'convert'; id: number; options: ConvertOptions; export: ExportOptions }
  | { type: 'geometry'; id: number; export: ExportOptions }
  | { type: 'export'; id: number; format: 'svg' | 'png'; options: ExportOptions }
  | { type: 'schedule'; id: number; durationMs: number };

export type Response =
  | {
      type: 'ready';
      defaults: ConvertOptions;
      /** Per pixel of font size: advance, line height, ascent, descent (negative). */
      metrics: [number, number, number, number];
    }
  | { type: 'loaded'; id: number; width: number; height: number; originalWidth: number; originalHeight: number; ms: number }
  | { type: 'converted'; id: number; text: string; cols: number; rows: number; ms: number; geometry: Geometry }
  | { type: 'geometry'; id: number; geometry: Geometry }
  | { type: 'score'; id: number; ssim: number; toneError: number }
  | { type: 'exported'; id: number; format: 'svg' | 'png'; data: string | Uint8Array }
  | { type: 'schedule'; id: number; times: Uint32Array }
  | { type: 'error'; id: number; message: string };
