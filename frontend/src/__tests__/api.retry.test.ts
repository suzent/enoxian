import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import type { ApiError as ApiErrorType } from '../api'

// api.ts reads window.__ENOX_TOKEN__ at module load, so set it before import.
;(window as unknown as { __ENOX_TOKEN__?: string }).__ENOX_TOKEN__ = 'test-token'

const { postChat, getChat, ApiError, inviteCircle, enterCircle } = await import('../api')

/** `circle_busy` mirrors what the daemon's `circle_busy()` really sends. */
function busy(): Response {
  return new Response(
    JSON.stringify({ error: 'Circle state is busy syncing. Try again shortly.', code: 'circle_busy' }),
    { status: 503, headers: { 'Retry-After': '1', 'content-type': 'application/json' } },
  )
}
function created(id = 'm1'): Response {
  return new Response(JSON.stringify({ id }), { status: 201, headers: { 'content-type': 'application/json' } })
}
function badRequest(): Response {
  return new Response(JSON.stringify({ error: 'text is required' }), {
    status: 400, headers: { 'content-type': 'application/json' },
  })
}

let fetchMock: ReturnType<typeof vi.fn>

beforeEach(() => {
  vi.useFakeTimers()
  fetchMock = vi.fn()
  vi.stubGlobal('fetch', fetchMock)
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

/** Run a promise to completion while fast-forwarding the retry backoff. */
async function settle<T>(p: Promise<T>): Promise<T> {
  const result = p.then(v => ({ ok: true as const, v }), e => ({ ok: false as const, e }))
  await vi.runAllTimersAsync()
  const r = await result
  if (!r.ok) throw r.e
  return r.v
}

describe('circle_busy retry', () => {
  it('retries a busy POST and succeeds, so a moment of contention is invisible', async () => {
    fetchMock.mockResolvedValueOnce(busy()).mockResolvedValueOnce(created('m42'))
    const res = await settle(postChat('c1', 'hello', 'alice'))
    expect(fetchMock).toHaveBeenCalledTimes(2)
    expect(res).toEqual({ id: 'm42' })
  })

  it('gives up after a bounded number of attempts rather than retrying forever', async () => {
    fetchMock.mockImplementation(async () => busy())
    await expect(settle(postChat('c1', 'hello', 'alice'))).rejects.toThrow(/busy syncing/)
    // 1 initial + 3 retries. A message that never sends must still surface.
    expect(fetchMock).toHaveBeenCalledTimes(4)
  })

  it('does NOT retry a real rejection — only the transient one', async () => {
    fetchMock.mockImplementation(async () => badRequest())
    await expect(settle(postChat('c1', '', 'alice'))).rejects.toThrow(/text is required/)
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('keeps the status and code on the error so callers can tell them apart', async () => {
    fetchMock.mockImplementation(async () => badRequest())
    const err = await settle(postChat('c1', '', 'alice')).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect((err as ApiErrorType).status).toBe(400)
    expect((err as ApiErrorType).code).toBeUndefined()
  })

  it('surfaces the daemon wording, not a generic HTTP line', async () => {
    fetchMock.mockImplementation(async () => busy())
    const err = await settle(getChat('c1')).catch((e: unknown) => e)
    expect((err as ApiErrorType).message).toBe('Circle state is busy syncing. Try again shortly.')
    expect((err as ApiErrorType).code).toBe('circle_busy')
  })

  it('retries reads too — a busy GET is the same transient refusal', async () => {
    fetchMock
      .mockResolvedValueOnce(busy())
      .mockResolvedValueOnce(new Response('[]', { status: 200, headers: { 'content-type': 'application/json' } }))
    const res = await settle(getChat('c1'))
    expect(fetchMock).toHaveBeenCalledTimes(2)
    expect(res).toEqual([])
  })
})


describe('invite request timeout', () => {
  it.each([
    ['generate', () => inviteCircle('c1')],
    ['join', () => enterCircle('enoxian://s1/test')],
  ])('lets %s wait for the relay beyond the ordinary JSON timeout', async (_name, start) => {
    let signal: AbortSignal | undefined
    let finish!: (value: Response) => void
    fetchMock.mockImplementation((_url: string, init: RequestInit) => {
      signal = init.signal as AbortSignal
      return new Promise<Response>(resolve => { finish = resolve })
    })
    const request = start()
    await vi.advanceTimersByTimeAsync(26_000)
    expect(signal?.aborted).toBe(false)
    finish(new Response('{}', { status: 200 }))
    await request
    expect(vi.getTimerCount()).toBe(0)
  })
})
