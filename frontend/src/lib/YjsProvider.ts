import * as Y from 'yjs'
import * as syncProtocol from 'y-protocols/sync'
import * as awarenessProtocol from 'y-protocols/awareness'
import * as encoding from 'lib0/encoding'
import * as decoding from 'lib0/decoding'

const MSG_SYNC = 0
const MSG_AWARENESS = 1

export class YjsProvider {
  public awareness: awarenessProtocol.Awareness
  private ws: WebSocket | null = null
  private destroyed = false
  private onSyncCallback: (() => void) | undefined

  constructor(
    private url: string,
    private doc: Y.Doc,
    onSync?: () => void,
  ) {
    this.awareness = new awarenessProtocol.Awareness(doc)
    this.onSyncCallback = onSync
    this.connect()
  }

  private connect() {
    if (this.destroyed) return
    const ws = new WebSocket(this.url)
    ws.binaryType = 'arraybuffer'
    this.ws = ws

    ws.onopen = () => {
      // Send our SyncStep1
      const enc = encoding.createEncoder()
      encoding.writeVarUint(enc, MSG_SYNC)
      syncProtocol.writeSyncStep1(enc, this.doc)
      ws.send(encoding.toUint8Array(enc))

      // Send initial awareness
      const aEnc = encoding.createEncoder()
      encoding.writeVarUint(aEnc, MSG_AWARENESS)
      encoding.writeVarUint8Array(
        aEnc,
        awarenessProtocol.encodeAwarenessUpdate(this.awareness, [this.doc.clientID]),
      )
      ws.send(encoding.toUint8Array(aEnc))
    }

    ws.onmessage = (e) => {
      const data = new Uint8Array(e.data as ArrayBuffer)
      const dec = decoding.createDecoder(data)
      const msgType = decoding.readVarUint(dec)

      if (msgType === MSG_SYNC) {
        const replyEnc = encoding.createEncoder()
        encoding.writeVarUint(replyEnc, MSG_SYNC)
        const syncType = syncProtocol.readSyncMessage(dec, replyEnc, this.doc, this)
        if (encoding.length(replyEnc) > 1) {
          ws.send(encoding.toUint8Array(replyEnc))
        }
        if (syncType === syncProtocol.messageYjsSyncStep2 && this.onSyncCallback) {
          this.onSyncCallback()
          this.onSyncCallback = undefined
        }
      } else if (msgType === MSG_AWARENESS) {
        const raw = decoding.readVarUint8Array(dec)
        console.debug('[yjs] recv awareness', raw.length, 'bytes, states before:', this.awareness.getStates().size)
        awarenessProtocol.applyAwarenessUpdate(this.awareness, raw, this)
        console.debug('[yjs] recv awareness applied, states after:', this.awareness.getStates().size, [...this.awareness.getStates().entries()])
      }
    }

    ws.onclose = () => {
      awarenessProtocol.removeAwarenessStates(this.awareness, [this.doc.clientID], this)
      if (!this.destroyed) setTimeout(() => this.connect(), 2000)
    }

    // Forward local doc updates to server
    const onUpdate = (update: Uint8Array, origin: unknown) => {
      if (origin === this || ws.readyState !== WebSocket.OPEN) return
      const enc = encoding.createEncoder()
      encoding.writeVarUint(enc, MSG_SYNC)
      syncProtocol.writeUpdate(enc, update)
      ws.send(encoding.toUint8Array(enc))
    }
    this.doc.on('update', onUpdate)

    // Forward awareness changes to server
    const onAwareness = ({ added, updated, removed }: { added: number[]; updated: number[]; removed: number[] }) => {
      if (ws.readyState !== WebSocket.OPEN) return
      const changed = [...added, ...updated, ...removed]
      const payload = awarenessProtocol.encodeAwarenessUpdate(this.awareness, changed)
      console.debug('[yjs] send awareness', payload.length, 'bytes, clients:', changed)
      const enc = encoding.createEncoder()
      encoding.writeVarUint(enc, MSG_AWARENESS)
      encoding.writeVarUint8Array(enc, payload)
      ws.send(encoding.toUint8Array(enc))
    }
    this.awareness.on('update', onAwareness)

    ws.addEventListener('close', () => {
      this.doc.off('update', onUpdate)
      this.awareness.off('update', onAwareness)
    }, { once: true })
  }

  destroy() {
    this.destroyed = true
    awarenessProtocol.removeAwarenessStates(this.awareness, [this.doc.clientID], this)
    this.awareness.destroy()
    this.ws?.close()
  }
}
