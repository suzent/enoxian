/**
 * Where per-Circle settings live, and what the panel says about them.
 *
 * The risk this covers is a misreading, not a crash: a "per-Circle" setting
 * reads like it belongs to the Circle — shared with its members — when it is
 * this device's own answer about that Circle. The panel has to say so.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'

const getAgentConfigFor = vi.fn()

vi.mock('../../api', () => ({
  getAgentConfigFor,
  getAgentPlugins: vi.fn(async () => ({ plugins: [] })),
  discoverAgents: vi.fn(async () => ({ agents: [] })),
  installAgentPlugin: vi.fn(),
  setEngagement: vi.fn(async () => ({ ok: true })),
  addAgent: vi.fn(),
  removeAgent: vi.fn(),
  getConnectivitySettings: vi.fn(async () => ({ force_relay: false })),
  getIdentity: vi.fn(async () => ({
    device_label: 'macbook-pro', user_handle: 'suzy', has_user_key: true, update_channel: 'stable',
  })),
  setIdentity: vi.fn(async () => ({ status: 'ok' })),
  setForceRelay: vi.fn(),
}))

vi.mock('../../context/AppContext', () => ({
  useApp: () => ({
    activeCircleId: 'c1',
    circles: [{ circle_id: 'c1', circle_name: 'group', disabled: false }],
  }),
}))

const { default: DeviceSettings } = await import('../DeviceSettings')

const CONFIG = {
  reaction: 'push',
  config_path: '/tmp/agents.toml',
  configured: true,
  global_settings: {
    reaction: 'push', engagement_window_secs: 180, ambient: [], max_relay_turns: 20,
  },
  circle: {
    circle_id: 'c1',
    overrides: {},
    effective: {
      reaction: 'push', engagement_window_secs: 180, ambient: [], max_relay_turns: 20,
    },
  },
  agents: [{
    name: 'claude', driver: 'acp', command: ['x'], working_dir: null,
    installed: true, status: 'ready',
  }],
}

beforeEach(() => {
  getAgentConfigFor.mockReset()
  getAgentConfigFor.mockResolvedValue(CONFIG)
})

describe('per-Circle settings', () => {
  it('are reachable from the same panel, under a named scope', async () => {
    render(<DeviceSettings onClose={vi.fn()} />)
    // The Circle is named on the tab, not called "this Circle" — you can only
    // tell which one you are editing if it says.
    expect(await screen.findByRole('tab', { name: 'GROUP' })).toBeTruthy()
    expect(screen.getByRole('tab', { name: 'ALL CIRCLES' })).toBeTruthy()
  })

  it('open on the global scope, since most people have one answer', async () => {
    render(<DeviceSettings onClose={vi.fn()} />)
    expect(await screen.findByText(/Applies in every Circle/)).toBeTruthy()
  })

  it('say plainly that a per-Circle setting is not shared with the Circle', async () => {
    render(<DeviceSettings onClose={vi.fn()} />)
    await userEvent.click(await screen.findByRole('tab', { name: 'GROUP' }))
    await waitFor(() => expect(screen.getByText(/Applies in/)).toBeTruthy())
    expect(screen.getByText(/never shared with the Circle/)).toBeTruthy()
  })

  it('ask the daemon for the active Circle, so overrides can be shown at all', async () => {
    render(<DeviceSettings onClose={vi.fn()} />)
    await waitFor(() => expect(getAgentConfigFor).toHaveBeenCalledWith('c1'))
  })
})
