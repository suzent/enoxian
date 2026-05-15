import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    port: 36520,
    proxy: {
      '/circles': {
        target: 'http://127.0.0.1:36521',
        ws: true,
      },
      '/shutdown': 'http://127.0.0.1:36521',
    },
  },
  build: {
    outDir: '../static',
    emptyOutDir: true,
  },
})
