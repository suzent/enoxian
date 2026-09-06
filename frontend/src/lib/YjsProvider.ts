import * as Y from 'yjs'
import * as syncProtocol from 'y-protocols/sync'
import * as awarenessProtocol from 'y-protocols/awareness'
import * as encoding from 'lib0/encoding'
import * as decoding from 'lib0/decoding'

const MSG_SYNC = 0
const MSG_AWARENESS = 1

// The daemon drops its SyncStep2 reply if the doc is locked by a peer or the
// file watcher when SyncStep1 arrives. That leaves the socket OPEN with no
// content and no error, so the reconnect-on-close path never runs and the
// editor sits on 'connecting' until the user reloads the page. Re-ask instead;
// SyncStep1 is idempotent. After the last attempt, close so the normal
// reconnect path takes over.
const SYNC_TIMEOUT_MS = 5_000
const SYNC_MAX_ATTEMPTS = 3

export type YjsConnectionStatus = 'connecting' | 'synced' | 'disconnected'

export class YjsProvider {
  public awareness: awarenessProtocol.Awareness
  private ws: WebSocket | null = null
  private destroyed = false
  private onSyncCallback: (() => void) | undefined
  private onStatusChange: ((status: YjsConnectionStatus) => void) | undefined
  private awarenessHeartbeat: number | undefined
  private synced = false
  private syncTimer: number | undefined
  private syncAttempts = 0
  // Distinct from `synced`, which also flips on the daemon's own SyncStep1
  // (it opens the handshake with one). Only SyncStep2/Update carry actual
  // state, so only those prove our SyncStep1 was answered.
  private stateReceived = false

  constructor(
    private url: string,
    private doc: Y.Doc,
    onSync?: () => void,
    onStatusChange?: (status: YjsConnectionStatus) => void,
  ) {
    this.awareness = new awarenessProtocol.Awareness(doc)
    this.onSyncCallback = onSync
    this.onStatusChange = onStatusChange
    this.emitStatus('connecting')
    // Defer connect to next microtask so the caller can set awareness state
    // (e.g. user name/color) before the initial awareness broadcast is sent.
    Promise.resolve().then(() => { if (!this.destroyed) this.connect() })
  }

  private emitStatus(status: YjsConnectionStatus) {
    this.onStatusChange?.(status)
  }

  private clearSyncTimeout() {
    if (this.syncTimer !== undefined) {
      window.clearTimeout(this.syncTimer)
      this.syncTimer = undefined
    }
  }

  /** Ask the daemon for its state. Safe to repeat: the reply is a full diff. */
  private sendSyncStep1(ws: WebSocket) {
    if (ws.readyState !== WebSocket.OPEN) return
    const enc = encoding.createEncoder()
    encoding.writeVarUint(enc, MSG_SYNC)
    syncProtocol.writeSyncStep1(enc, this.doc)
    ws.send(encoding.toUint8Array(enc))
    this.armSyncTimeout(ws)
  }

  private armSyncTimeout(ws: WebSocket) {
    this.clearSyncTimeout()
    this.syncTimer = window.setTimeout(() => {
      if (this.destroyed || this.stateReceived || ws.readyState !== WebSocket.OPEN) return
      this.syncAttempts += 1
      if (this.syncAttempts >= SYNC_MAX_ATTEMPTS) {
        console.warn('[yjs] no sync reply after retries; reconnecting', this.url)
        ws.close()
        return
      }
      this.sendSyncStep1(ws)
    }, SYNC_TIMEOUT_MS)
  }

  private markSynced() {
    if (this.synced) return
    this.synced = true
    this.onSyncCallback?.()
    this.onSyncCallback = undefined
    this.emitStatus('synced')
  }

  private sendAwarenessUpdate(clientIds: number[], ws = this.ws) {
    if (!ws || ws.readyState !== WebSocket.OPEN || clientIds.length === 0) return
    const payload = awarenessProtocol.encodeAwarenessUpdate(this.awareness, clientIds)
    const enc = encoding.createEncoder()
    encoding.writeVarUint(enc, MSG_AWARENESS)
    encoding.writeVarUint8Array(enc, payload)
    ws.send(encoding.toUint8Array(enc))
  }

  private closeAfterBufferedSend(ws = this.ws) {
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      ws?.close()
      return
    }
    const deadline = Date.now() + 150
    const closeWhenFlushed = () => {
      if (ws.readyState !== WebSocket.OPEN) return
      if (ws.bufferedAmount === 0 || Date.now() >= deadline) {
        ws.close()
        return
      }
      window.setTimeout(closeWhenFlushed, 10)
    }
    closeWhenFlushed()
  }

  private connect() {
    if (this.destroyed) return
    this.emitStatus('connecting')
    this.syncAttempts = 0
    this.stateReceived = false
    const ws = new WebSocket(this.url)
    ws.binaryType = 'arraybuffer'
    this.ws = ws

    ws.onopen = () => {
      this.sendSyncStep1(ws)

      // Send initial awareness
      this.sendAwarenessUpdate([this.doc.clientID], ws)
      if (this.awarenessHeartbeat !== undefined) window.clearInterval(this.awarenessHeartbeat)
      this.awarenessHeartbeat = window.setInterval(() => {
        this.sendAwarenessUpdate([this.doc.clientID], ws)
      }, 10_000)

    }

    ws.onmessage = (e) => {
      const data = new Uint8Array(e.data as ArrayBuffer)
      const dec = decoding.createDecoder(data)
      const msgType = decoding.readVarUint(dec)

      if (msgType === MSG_SYNC) {
        const replyEnc = encoding.createEncoder()
        encoding.writeVarUint(replyEnc, MSG_SYNC)
        const syncType = syncProtocol.readSyncMessage(dec, replyEnc, this.doc, this)
        if (encoding.length(replyEnc) > 1 && ws.readyState === WebSocket.OPEN) {
          ws.send(encoding.toUint8Array(replyEnc))
        }
        if (
          syncType === syncProtocol.messageYjsSyncStep2 ||
          syncType === syncProtocol.messageYjsUpdate
        ) {
          // The daemon answered with real state — stop asking.
          this.stateReceived = true
          this.clearSyncTimeout()
        }
        if (
          syncType === syncProtocol.messageYjsSyncStep1 ||
          syncType === syncProtocol.messageYjsSyncStep2 ||
          syncType === syncProtocol.messageYjsUpdate
        ) {
          this.markSynced()
        }
      } else if (msgType === MSG_AWARENESS) {
        const raw = decoding.readVarUint8Array(dec)
        awarenessProtocol.applyAwarenessUpdate(this.awareness, raw, this)
      }
    }

    ws.onclose = () => {
      this.clearSyncTimeout()
      if (!this.destroyed) {
        this.synced = false
        this.stateReceived = false
        this.emitStatus('disconnected')
        setTimeout(() => this.connect(), 2000)
      }
    }

    ws.onerror = (event) => {
      if (!this.destroyed) {
        this.synced = false
        console.warn('[yjs] websocket error', this.url, event)
        this.emitStatus('disconnected')
      }
    }

    const onUpdate = (update: Uint8Array, origin: unknown) => {
      if (origin === this || ws.readyState !== WebSocket.OPEN) return
      const enc = encoding.createEncoder()
      encoding.writeVarUint(enc, MSG_SYNC)
      syncProtocol.writeUpdate(enc, update)
      ws.send(encoding.toUint8Array(enc))
    }
    this.doc.on('update', onUpdate)

    const onAwareness = ({ added, updated, removed }: { added: number[]; updated: number[]; removed: number[] }) => {
      const changed = [...added, ...updated, ...removed]
      this.sendAwarenessUpdate(changed, ws)
    }
    this.awareness.on('update', onAwareness)

    ws.addEventListener('close', () => {
      this.doc.off('update', onUpdate)
      this.awareness.off('update', onAwareness)
      if (this.awarenessHeartbeat !== undefined) {
        window.clearInterval(this.awarenessHeartbeat)
        this.awarenessHeartbeat = undefined
      }
    }, { once: true })
  }

  destroy() {
    this.destroyed = true
    this.clearSyncTimeout()
    if (this.awarenessHeartbeat !== undefined) {
      window.clearInterval(this.awarenessHeartbeat)
      this.awarenessHeartbeat = undefined
    }
    awarenessProtocol.removeAwarenessStates(this.awareness, [this.doc.clientID], this)
    this.awareness.destroy()
    this.closeAfterBufferedSend()
  }
}
