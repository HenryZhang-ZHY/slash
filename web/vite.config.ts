/// <reference types="vitest/config" />

import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'node:path'

const collectorToken = process.env.BUILDKITE_ANALYTICS_TOKEN

const testReporters = collectorToken
  ? ['default' as const, 'buildkite-test-collector/vitest/reporter' as const]
  : ['default' as const]

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(import.meta.dirname, './src'),
    },
  },
  test: {
    reporters: testReporters,
    includeTaskLocation: true,
  },
  server: {
    proxy: {
      // In dev, forward API calls to the Rust backend (slash-server).
      '/api': {
        target: 'http://localhost:8080',
        changeOrigin: true,
      },
    },
  },
})
