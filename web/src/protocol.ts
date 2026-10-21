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
  fontSize: number;
  frame: boolean;
  title: string;
  animate: boolean;
  revealMs: number;
  loopPauseMs?: number;
  svgText: boolean;
};

export type Request =
  | { type: 'load'; id: number; file: Blob }
  | { type: 'convert'; id: number; options: ConvertOptions }
  | { type: 'export'; id: number; format: 'svg' | 'png'; options: ExportOptions }
  | { type: 'schedule'; id: number; durationMs: number };

export type Response =
  | {
      type: 'ready';
      defaults: ConvertOptions;
      /** Cell advance and line height per pixel of font size. */
      cell: [number, number];
      palettes: Record<'dark' | 'light', [string, string]>;
    }
  | { type: 'loaded'; id: number; width: number; height: number; ms: number }
  | { type: 'converted'; id: number; text: string; cols: number; rows: number; ms: number }
  | { type: 'score'; id: number; ssim: number; toneError: number }
  | { type: 'exported'; id: number; format: 'svg' | 'png'; data: string | Uint8Array }
  | { type: 'schedule'; id: number; times: Uint32Array }
  | { type: 'error'; id: number; message: string };
