import { describe, expect, it, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react'
import type { AgentConfigView } from '../../types'
import type { WardenView } from '../../api'

const config = (over: Partial<AgentConfigView['global_settings']> = {}, seen = 0): AgentConfigView => ({
  reaction: 'pull', circle: null, config_path: '', configured: true, recommended_seen: seen,
  agents: [
    { name: 'claude', driver: 'acp', command: ['claude'], installed: true, status: 'ready' },
    { name: 'codex', driver: 'acp', command: ['codex'], installed: false, status: 'missing' },
  ] as AgentConfigView['agents'],
  global_settings: {
    reaction: 'pull', engagement_window_secs: 0, ambient: [], max_relay_turns: 20, warden_mode: 'off', ambient_responders: 1,
    ...over,
  } as AgentConfigView['global_settings'],
})
const warden = (hint: string | null): WardenView => ({
  mode: 'off', config: { decider: 'jev', timeout_secs: 5 },
  kinds: [{ id: 'jev', name: 'TypeSafe Jev', description: '', protocol: 'systemone', needs_key: true, builtin: true, fields: {}, key_hint: hint }],
} as unknown as WardenView)

const getAgentConfig = vi.fn()
const getWarden = vi.fn()
const setEngagement = vi.fn(async () => ({ ok: true }))
vi.mock('../../api', () => ({ getAgentConfig, getWarden, setEngagement }))

const { default: RecommendedSetup, recommendation } = await import('../RecommendedSetup')

describe('recommendation', () => {
  it('turns the Warden on only when it has a key to decide with', () => {
    const without = recommendation(config(), warden(null))
    expect(without.patch).toEqual({ reaction: 'push', engagement_window_secs: 180, ambient: ['claude'] })
    expect(without.rows.find(r => r.label === 'Warden')?.blocked).toBe(true)

    const withKey = recommendation(config(), warden('••••cret'))
    expect(withKey.patch).toMatchObject({ warden_mode: 'act', ambient_responders: 32 })
    expect(withKey.rows.some(r => r.blocked)).toBe(false)
  })

  it('leaves the Warden off when the daemon cannot use the decider it has', () => {
    const broken = { ...warden('••••cret'), problem: 'JEV_KEY is not set' }
    const { patch, rows } = recommendation(config(), broken)
    expect(patch.warden_mode).toBeUndefined()
    expect(patch.ambient_responders).toBeUndefined()
    expect(rows.find(r => r.label === 'Warden')).toMatchObject({ blocked: true, to: 'Fix its decider under Settings → Warden first' })
  })

  it('keeps agents already reading the room and lists nothing already recommended', () => {
    const { patch, rows } = recommendation(config({
      reaction: 'push', engagement_window_secs: 180, ambient: ['gemini', 'claude'], warden_mode: 'act', ambient_responders: 32,
    }), warden('••••cret'))
    expect(rows).toEqual([])
    expect(patch.ambient).toBeUndefined()
  })
})

describe('RecommendedSetup', () => {
  beforeEach(() => { cleanup(); vi.clearAllMocks() })

  it('applies the recommendation and remembers the answer', async () => {
    getAgentConfig.mockResolvedValue(config())
    getWarden.mockResolvedValue(warden(null))
    const close = vi.fn()
    render(<RecommendedSetup auto onClose={close} />)
    expect(await screen.findByText('Waits until fetched')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'USE RECOMMENDED' }))
    await waitFor(() => expect(close).toHaveBeenCalledWith(true))
    expect(setEngagement).toHaveBeenCalledWith({
      reaction: 'push', engagement_window_secs: 180, ambient: ['claude'], recommended_seen: 1,
    })
  })

  it('keeps everything as it was but still remembers the answer', async () => {
    getAgentConfig.mockResolvedValue(config())
    getWarden.mockResolvedValue(warden(null))
    const close = vi.fn()
    render(<RecommendedSetup auto onClose={close} />)
    fireEvent.click(await screen.findByRole('button', { name: 'KEEP AS IS' }))
    await waitFor(() => expect(close).toHaveBeenCalledWith(false))
    expect(setEngagement).toHaveBeenCalledWith({ recommended_seen: 1 })
  })

  it('does not ask a device that already answered', async () => {
    getAgentConfig.mockResolvedValue(config({}, 1))
    getWarden.mockResolvedValue(warden(null))
    const close = vi.fn()
    render(<RecommendedSetup auto onClose={close} />)
    await waitFor(() => expect(close).toHaveBeenCalledWith(false))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(setEngagement).not.toHaveBeenCalled()
  })

  it('can still be opened from settings after answering', async () => {
    getAgentConfig.mockResolvedValue(config({}, 1))
    getWarden.mockResolvedValue(warden(null))
    render(<RecommendedSetup onClose={vi.fn()} />)
    expect(await screen.findByRole('button', { name: 'USE RECOMMENDED' })).toBeTruthy()
  })
})
