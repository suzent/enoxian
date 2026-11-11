import { renderHook, act, waitFor } from '@testing-library/react'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import type { AgentAuthState } from '../../api'

const api = vi.hoisted(() => ({ getAgentAuth: vi.fn(), startAgentAuth: vi.fn(), cancelAgentAuth: vi.fn() }))
vi.mock('../../api', () => api)
import { useAgentAuthentication } from '../AgentAuthentication'

const ready: AgentAuthState = { status: 'ready', methods: [], error: null }

describe('useAgentAuthentication', () => {
  beforeEach(() => { vi.useFakeTimers({ shouldAdvanceTime: true }); Object.values(api).forEach(fn => fn.mockReset()) })
  afterEach(() => { vi.useRealTimers() })

  it('retries a first check that could not start', async () => {
    api.getAgentAuth.mockResolvedValue({})
    api.startAgentAuth.mockRejectedValueOnce(new Error('too many checks')).mockResolvedValue(ready)
    const { result } = renderHook(() => useAgentAuthentication(true))
    await waitFor(() => expect(api.getAgentAuth).toHaveBeenCalled())
    await act(async () => { await vi.advanceTimersByTimeAsync(0) })
    await act(async () => { result.current.autoCheck('codex'); await vi.advanceTimersByTimeAsync(0) })
    expect(api.startAgentAuth).toHaveBeenCalledTimes(1)
    await act(async () => { await vi.advanceTimersByTimeAsync(3_000) })
    expect(api.startAgentAuth).toHaveBeenCalledTimes(2)
    expect(api.startAgentAuth).toHaveBeenLastCalledWith('codex', 'probe', undefined, undefined)
  })

  it('times a signed-in result from when it was first polled, not from the next return', async () => {
    api.getAgentAuth.mockResolvedValue({ codex: ready })
    api.startAgentAuth.mockResolvedValue(ready)
    renderHook(() => useAgentAuthentication(true))
    await waitFor(() => expect(api.getAgentAuth).toHaveBeenCalled())
    await act(async () => { await vi.advanceTimersByTimeAsync(5 * 60_000 + 1_000) })
    act(() => { window.dispatchEvent(new Event('focus')) })
    expect(api.startAgentAuth).toHaveBeenCalledWith('codex', 'probe', undefined, undefined)
  })
})
