import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

// In dev, enoxd requires the local API token on every request. The daemon
// injects it into its own served HTML, but Vite serves the frontend from source
// without that injection — so the dev proxy reads the token from disk and adds
// the Authorization header (and a ?token= for WS) to proxied requests. Keeps the
// dev workflow working against a hardened daemon. See docs/reference/daemon.md.
function apiToken(): string | null {
  try {
    return readFileSync(join(homedir(), '.enoxian', 'api.token'), 'utf8').trim() || null
  } catch {
    return null
  }
}

const TOKEN = apiToken()
const target = 'http://127.0.0.1:36521'
const authHeaders = TOKEN ? { Authorization: `Bearer ${TOKEN}` } : undefined

// Inject the token into the dev HTML the same way the daemon does for its
// production HTML, so the frontend's window.__ENOX_TOKEN__ is set — this is what
// authenticates WebSocket/SSE (via ?token=) in dev.
function injectToken() {
  return {
    name: 'inject-enox-token',
    transformIndexHtml(html: string) {
      if (!TOKEN) return html
      return html.replace(
        '</head>',
        `<script>window.__ENOX_TOKEN__=${JSON.stringify(TOKEN)};</script></head>`,
      )
    },
  }
}

export default defineConfig({
  plugins: [react(), injectToken()],
  server: {
    port: 36520,
    proxy: {
      '/circles': {
        target,
        ws: true,
        headers: authHeaders,
      },
      '/api': { target, headers: authHeaders },
      '/shutdown': { target, headers: authHeaders },
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
