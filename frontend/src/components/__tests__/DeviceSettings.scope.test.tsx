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
  getAgentCards: vi.fn(async () => ({ cards: [] })),
  getWarden: vi.fn(async () => ({
    mode: 'off', config: { decider: 'builtin:rules', timeout_secs: 3 }, problem: null,
    kinds: [{ id: 'builtin:rules', name: 'Built-in rules', description: '', protocol: 'rules', needs_key: false, builtin: true, fields: {} }],
    spec_toml: 'version = "builtin-1"\n', spec: { version: 'builtin-1', modes: [], admit_threshold: 0.5, lambda: 0.05 },
    spec_source: { kind: 'default' },
  })),
  setWardenConfig: vi.fn(), setWardenKey: vi.fn(), setWardenSpec: vi.fn(), resetWardenSpec: vi.fn(), testWarden: vi.fn(),
  getAgentAuth: vi.fn(async () => ({})),
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

describe('settings recovery', () => {
  it('keeps the numeric field mounted during a save', async () => {
    const { setEngagement } = await import('../../api')
    render(<DeviceSettings onClose={vi.fn()} />)
    const input = await screen.findByLabelText('Maximum agent turns in one chain')
    fireEvent.change(input, { target: { value: '32' } })
    expect(setEngagement).not.toHaveBeenCalled()
    fireEvent.blur(input)
    await waitFor(() => expect(setEngagement).toHaveBeenCalledWith({ max_relay_turns: 32 }))
    expect(screen.getByLabelText('Maximum agent turns in one chain')).toBe(input)
  })
  it('retries the connectivity request, not an unrelated configuration request', async () => {
    const { getConnectivitySettings } = await import('../../api')
    vi.mocked(getConnectivitySettings).mockRejectedValueOnce(new Error('Connection unavailable'))
    render(<DeviceSettings circleId="c1" onClose={vi.fn()} />)
    await userEvent.click(screen.getByRole('tab', { name: 'Connectivity' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Connection unavailable')
    await userEvent.click(screen.getByRole('button', { name: 'TRY AGAIN' }))
    expect(await screen.findByTitle('https://relay.enoxian.com/')).toBeTruthy()
    expect(getConnectivitySettings).toHaveBeenCalledTimes(2)
  })
  it('shows identity failures and can recover', async () => {
    const { getIdentity } = await import('../../api')
    vi.mocked(getIdentity).mockRejectedValueOnce(new Error('Identity unavailable'))
    render(<DeviceSettings onClose={vi.fn()} />)
    await userEvent.click(screen.getByRole('tab', { name: 'Device' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Identity unavailable')
    await userEvent.click(screen.getByRole('button', { name: 'TRY AGAIN' }))
    expect(await screen.findByLabelText('Device name')).toHaveValue('macbook-pro')
  })
  it('preserves a custom agent draft after the server rejects it', async () => {
    const { addAgent } = await import('../../api')
    vi.mocked(addAgent).mockRejectedValueOnce(new Error('Agent already exists'))
    render(<DeviceSettings onClose={vi.fn()} />)
    await userEvent.click(screen.getByRole('tab', { name: 'Agents' }))
    await userEvent.click(await screen.findByText('ADVANCED'))
    await userEvent.click(screen.getByTitle('Add a custom agent'))
    await userEvent.type(screen.getByLabelText('Agent name'), 'example')
    await userEvent.type(screen.getByLabelText('Executable and arguments'), 'example --acp')
    await userEvent.click(screen.getByRole('button', { name: 'ADD' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Agent already exists')
    expect(screen.getByLabelText('Agent name')).toHaveValue('example')
    expect(screen.getByLabelText('Executable and arguments')).toHaveValue('example --acp')
  })
  it('asks before discarding a device identity draft', async () => {
    const close = vi.fn()
    render(<DeviceSettings onClose={close} />)
    await userEvent.click(screen.getByRole('tab', { name: 'Device' }))
    await userEvent.type(await screen.findByLabelText('Device name'), '-changed')
    await userEvent.keyboard('{Escape}')
    expect(screen.getByRole('dialog', { name: 'Discard unsaved changes?' })).toBeTruthy()
    expect(close).not.toHaveBeenCalled()
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(screen.getByLabelText('Device name')).toHaveValue('macbook-pro-changed')
  })
})

describe('separate settings entry points', () => {
  it('gives Warden a dedicated global page and protects unsaved policy edits', async () => {
    const close = vi.fn()
    render(<DeviceSettings onClose={close} />)
    await userEvent.click(screen.getByRole('tab', { name: 'Warden' }))
    await userEvent.click(await screen.findByText('Advanced policy'))
    fireEvent.change(screen.getByLabelText('Warden spec'), { target: { value: 'new policy' } })
    await userEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(screen.getByRole('dialog', { name: 'Discard unsaved changes?' })).toBeTruthy()
    expect(close).not.toHaveBeenCalled()
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(screen.getByLabelText('Warden spec')).toHaveValue('new policy')
  })
  it('opens global settings without a scope picker', async () => {
    render(<DeviceSettings onClose={vi.fn()} />)
    expect(await screen.findByText(/Defaults for all Circles/)).toBeTruthy()
    expect(screen.queryByRole('combobox', { name: 'Settings scope' })).toBeNull()
    expect(screen.getByRole('tab', { name: 'Device' })).toBeTruthy()
    expect(screen.getByRole('tab', { name: 'Agents' })).toBeTruthy()
    expect(screen.queryByRole('tab', { name: 'Connectivity' })).toBeNull()
    expect(getAgentConfigFor).toHaveBeenCalledWith(null)
  })

  it('loads the requested Circle independently of the active chat', async () => {
    render(<DeviceSettings circleId="c2" onClose={vi.fn()} />)
    await waitFor(() => expect(getAgentConfigFor).toHaveBeenCalledWith('c2'))
    expect(screen.queryByRole('tab', { name: 'Device' })).toBeNull()
    expect(screen.queryByRole('tab', { name: 'Agents' })).toBeNull()
    expect(screen.getByRole('tab', { name: 'Connectivity' })).toBeTruthy()
  })

  it('names the Circle and explains its settings are private', async () => {
    render(<DeviceSettings circleId="c1" onClose={vi.fn()} />)
    expect(await screen.findByText(/Only affects this device/)).toHaveTextContent('global defaults')
    expect(screen.getByText(/never shared with the Circle/)).toBeTruthy()
    expect(screen.getByRole('dialog', { name: 'Circle settings' })).toBeTruthy()
    expect(screen.queryByRole('combobox', { name: 'Settings scope' })).toBeNull()
  })

  it('offers connectivity directly in Circle settings', async () => {
    render(<DeviceSettings circleId="c1" onClose={vi.fn()} />)
    await screen.findByText(/Only affects this device/)
    await userEvent.click(screen.getByRole('tab', { name: 'Connectivity' }))
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
    await userEvent.click(screen.getByRole('tab', { name: 'Membership' }))
    await userEvent.click(screen.getByRole('button', { name: 'Leave Circle…' }))
    expect(leave).toHaveBeenCalledOnce()
    expect(screen.getByRole('heading', { name: 'Circle membership' })).toBeTruthy()
  })

})
