import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The desktop UI lives in app/ and is built to dist/, which Tauri bundles.
export default defineConfig({
  root: 'app',
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { outDir: '../dist', emptyOutDir: true, target: 'es2022' },
});
