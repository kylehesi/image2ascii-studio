import { defineConfig } from 'vite';

export default defineConfig({
  // Relative asset URLs: the build works from any sub-path (GitHub Pages
  // serves it under /<repo>/) without hard-coding the repository name.
  base: './',
  // The bundled font lives in ../assets, shared with the Rust crates.
  server: { fs: { allow: ['..'] } },
  worker: { format: 'es' },
  build: { target: 'es2022' },
});
