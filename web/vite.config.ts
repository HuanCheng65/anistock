import { readFileSync } from 'node:fs'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

// the app's version is the server's (backend/Cargo.toml): a mismatch at runtime means a new release
const cargo = readFileSync(new URL('../backend/Cargo.toml', import.meta.url), 'utf8')
const version = /^version\s*=\s*"([^"]+)"/m.exec(cargo)?.[1] ?? 'dev'

export default defineConfig({
  plugins: [vue()],
  define: { __APP_VERSION__: JSON.stringify(version) },
  server: {
    proxy: { '/api': { target: 'http://127.0.0.1:18620', ws: true } },
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    chunkSizeWarningLimit: 900,
  },
})
