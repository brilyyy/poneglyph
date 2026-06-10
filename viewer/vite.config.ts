import { defineConfig } from 'vite'
import { devtools } from '@tanstack/devtools-vite'

import { tanstackRouter } from '@tanstack/router-plugin/vite'

import viteReact from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Dev: `poneglyph serve` (HTTP on 3742) + `pnpm dev` (this proxy).
// Build: `vite build` → dist/, embedded into the binary via rust-embed
// (cargo feature `embed-viewer`; see scripts/build-release.sh).
const config = defineConfig({
  resolve: { tsconfigPaths: true },
  plugins: [
    devtools(),
    tailwindcss(),
    tanstackRouter({ target: 'react', autoCodeSplitting: true }),
    viteReact(),
  ],
  build: { outDir: 'dist' },
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:3742',
      '/ingest': 'http://127.0.0.1:3742',
    },
  },
})

export default config
