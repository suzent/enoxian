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
      '/api': 'http://127.0.0.1:36521',
      '/shutdown': 'http://127.0.0.1:36521',
    },
  },
  build: {
    outDir: '../static',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes('node_modules')) return
          if (id.includes('three')) return 'vendor-three'
          if (id.includes('@codemirror') || id.includes('y-codemirror.next')) return 'vendor-editor'
          if (id.includes('yjs') || id.includes('y-protocols') || id.includes('lib0')) return 'vendor-yjs'
          if (id.includes('react') || id.includes('react-dom')) return 'vendor-react'
          return 'vendor'
        },
      },
    },
  },
})
