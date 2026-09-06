import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import * as Y from 'yjs'
import * as encoding from 'lib0/encoding'
import * as decoding from 'lib0/decoding'
import * as syncProtocol from 'y-protocols/sync'
import { YjsProvider } from '../YjsProvider'

const MSG_SYNC = 0

/** Minimal WebSocket stand-in: records sends, never replies on its own. */
class FakeWebSocket {
  static OPEN = 1
  static instances: FakeWebSocket[] = []
  readyState = FakeWebSocket.OPEN
  binaryType = ''
  sent: Uint8Array[] = []
  closed = false
  onopen: (() => void) | null = null
  onmessage: ((e: { data: ArrayBuffer }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: ((e: unknown) => void) | null = null
  private closeListeners: (() => void)[] = []

  constructor(public url: string) {
    FakeWebSocket.instances.push(this)
  }
  send(data: Uint8Array) { this.sent.push(new Uint8Array(data)) }
  addEventListener(type: string, fn: () => void) {
    if (type === 'close') this.closeListeners.push(fn)
  }
  close() {
    if (this.closed) return
    this.closed = true
    this.readyState = 3
    this.closeListeners.forEach(fn => fn())
    this.onclose?.()
  }
  /** Count of SyncStep1 frames the provider has sent. */
  syncStep1Count() {
    return this.sent.filter(buf => {
      const dec = decoding.createDecoder(buf)
      if (decoding.readVarUint(dec) !== MSG_SYNC) return false
      return decoding.readVarUint(dec) === syncProtocol.messageYjsSyncStep1
    }).length
  }
  /** Deliver the daemon's opening SyncStep1 (its state vector). */
  sendSyncStep1(doc: Y.Doc) {
    const enc = encoding.createEncoder()
    encoding.writeVarUint(enc, MSG_SYNC)
    syncProtocol.writeSyncStep1(enc, doc)
    this.onmessage?.({ data: encoding.toUint8Array(enc).buffer as ArrayBuffer })
  }

  /** Deliver a SyncStep2 carrying `doc`'s state, as the daemon would. */
  replyWithSyncStep2(doc: Y.Doc) {
    const enc = encoding.createEncoder()
    encoding.writeVarUint(enc, MSG_SYNC)
    syncProtocol.writeSyncStep2(enc, doc)
    this.onmessage?.({ data: encoding.toUint8Array(enc).buffer as ArrayBuffer })
  }
}

describe('YjsProvider initial sync recovery', () => {
  beforeEach(() => {
    FakeWebSocket.instances = []
    vi.stubGlobal('WebSocket', FakeWebSocket)
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  async function openProvider() {
    const doc = new Y.Doc()
    const onSync = vi.fn()
    const provider = new YjsProvider('ws://test/doc', doc, onSync)
    await vi.advanceTimersByTimeAsync(0) // constructor defers connect a microtask
    const ws = FakeWebSocket.instances[0]
    ws.onopen?.()
    return { doc, provider, ws, onSync }
  }

  it('re-sends SyncStep1 when the daemon never answers', async () => {
    const { ws, provider } = await openProvider()
    expect(ws.syncStep1Count()).toBe(1)

    await vi.advanceTimersByTimeAsync(5_000)
    expect(ws.syncStep1Count()).toBe(2)

    await vi.advanceTimersByTimeAsync(5_000)
    expect(ws.syncStep1Count()).toBe(3)

    provider.destroy()
  })

  it('closes the socket after the retry budget so the reconnect path runs', async () => {
    const { ws, provider } = await openProvider()
    await vi.advanceTimersByTimeAsync(15_000)
    expect(ws.closed).toBe(true)
    provider.destroy()
  })

  it("keeps retrying when the daemon's own SyncStep1 arrives but no state does", async () => {
    // The daemon opens the handshake with a SyncStep1 of its own. That is not
    // an answer to ours, so it must not call off the retry — otherwise a
    // dropped SyncStep2 leaves the editor empty with no recovery.
    const { ws, provider } = await openProvider()
    ws.sendSyncStep1(new Y.Doc())
    expect(ws.syncStep1Count()).toBe(1)

    await vi.advanceTimersByTimeAsync(5_000)
    expect(ws.syncStep1Count()).toBe(2)

    provider.destroy()
  })

  it('stops retrying once the daemon replies', async () => {
    const { ws, onSync, provider } = await openProvider()
    const remote = new Y.Doc()
    remote.getText('f').insert(0, 'hello')

    ws.replyWithSyncStep2(remote)
    expect(onSync).toHaveBeenCalledTimes(1)

    const after = ws.syncStep1Count()
    await vi.advanceTimersByTimeAsync(20_000)
    expect(ws.syncStep1Count()).toBe(after)
    expect(ws.closed).toBe(false)

    provider.destroy()
  })
})
