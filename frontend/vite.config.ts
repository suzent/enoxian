import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/circles': {
        target: 'http://127.0.0.1:9090',
        ws: true,
      },
      '/shutdown': 'http://127.0.0.1:9090',
    },
  },
  build: {
    outDir: '../static',
    emptyOutDir: true,
  },
})
