import { defineConfig } from 'vite';

export default defineConfig({
  // The bundled font lives in ../assets, shared with the Rust crates.
  server: { fs: { allow: ['..'] } },
  worker: { format: 'es' },
  build: { target: 'es2022' },
});
