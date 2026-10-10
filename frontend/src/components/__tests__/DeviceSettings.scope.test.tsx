/**
 * Where per-Circle settings live, and what the panel says about them.
 *
 * The risk this covers is a misreading, not a crash: a "per-Circle" setting
 * reads like it belongs to the Circle — shared with its members — when it is
 * this device's own answer about that Circle. The panel has to say so.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
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
  getConnectivitySettings: vi.fn(async () => ({ force_relay: false, active: true, relays: ['https://relay.enoxian.com/'] })),
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
  vi.clearAllMocks()
  getAgentConfigFor.mockReset()
  getAgentConfigFor.mockResolvedValue(CONFIG)
})

describe('separate settings entry points', () => {
  it('opens global settings without a scope picker', async () => {
    render(<DeviceSettings onClose={vi.fn()} />)
    expect(await screen.findByText(/Applies in every Circle/)).toBeTruthy()
    expect(screen.queryByRole('combobox', { name: 'Settings scope' })).toBeNull()
    expect(screen.getByRole('tab', { name: 'DEVICE' })).toBeTruthy()
    expect(screen.getByRole('tab', { name: 'AGENTS' })).toBeTruthy()
    expect(screen.queryByRole('tab', { name: 'CONNECTIVITY' })).toBeNull()
    expect(getAgentConfigFor).toHaveBeenCalledWith(null)
  })

  it('loads the requested Circle independently of the active chat', async () => {
    render(<DeviceSettings circleId="c2" onClose={vi.fn()} />)
    await waitFor(() => expect(getAgentConfigFor).toHaveBeenCalledWith('c2'))
    expect(screen.queryByRole('tab', { name: 'DEVICE' })).toBeNull()
    expect(screen.queryByRole('tab', { name: 'AGENTS' })).toBeNull()
    expect(screen.getByRole('tab', { name: 'CONNECTIVITY' })).toBeTruthy()
  })

  it('names the Circle and explains its settings are private', async () => {
    render(<DeviceSettings circleId="c1" onClose={vi.fn()} />)
    expect(await screen.findByText(/Applies in/)).toHaveTextContent('group')
    expect(screen.getByText(/never shared with the Circle/)).toBeTruthy()
    expect(screen.getByText('group · SETTINGS')).toBeTruthy()
    expect(screen.queryByRole('combobox', { name: 'Settings scope' })).toBeNull()
  })

  it('offers connectivity directly in Circle settings', async () => {
    render(<DeviceSettings circleId="c1" onClose={vi.fn()} />)
    await screen.findByText(/Applies in/)
    await userEvent.click(screen.getByRole('tab', { name: 'CONNECTIVITY' }))
    expect(await screen.findByText('Connectivity', { selector: 'h2' })).toBeTruthy()
    expect(await screen.findByTitle('https://relay.enoxian.com/')).toBeTruthy()
    expect(screen.getByText(/relay\.enoxian\.com/)).toBeTruthy()
  })
  it.each([undefined, 'c1'])('saves behaviour to the scope opened by the entry point (%s)', async circleId => {
    const { setEngagement } = await import('../../api')
    render(<DeviceSettings circleId={circleId} onClose={vi.fn()} />)
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Run agents when mentioned' }))
    await waitFor(() => expect(setEngagement).toHaveBeenCalledWith(
      circleId ? { circle_id: circleId, reaction: 'pull' } : { reaction: 'pull' },
    ))
  })

  it('groups Circle membership controls under a dedicated settings section', async () => {
    const leave = vi.fn()
    render(<DeviceSettings circleId="c1" onClose={vi.fn()} membership={<button onClick={leave}>Leave Circle…</button>} />)
    expect(screen.queryByRole('button', { name: 'Leave Circle…' })).toBeNull()
    await userEvent.click(screen.getByRole('tab', { name: 'MEMBERSHIP' }))
    await userEvent.click(screen.getByRole('button', { name: 'Leave Circle…' }))
    expect(leave).toHaveBeenCalledOnce()
    expect(screen.getByRole('heading', { name: 'Circle membership' })).toBeTruthy()
  })

})
