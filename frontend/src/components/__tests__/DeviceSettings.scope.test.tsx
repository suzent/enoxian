/**
 * Where per-Circle settings live, and what the panel says about them.
 *
 * The risk this covers is a misreading, not a crash: a "per-Circle" setting
 * reads like it belongs to the Circle — shared with its members — when it is
 * this device's own answer about that Circle. The panel has to say so.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'

const getAgentConfigFor = vi.fn()

vi.mock('../../api', () => ({
  getAgentConfigFor,
  getAgentPlugins: vi.fn(async () => ({ plugins: [] })),
  getAgentIcons: vi.fn(async () => ({ icons: {} })),
  agentIconUrl: (id: string) => `/api/agent-icons/${id}.svg`,
  discoverAgents: vi.fn(async () => ({ agents: [] })),
  installAgentPlugin: vi.fn(),
  uninstallAgentPlugin: vi.fn(),
  refreshAgentRegistry: vi.fn(),
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
    const user = userEvent.setup()
    let finishSave!: () => void
    vi.mocked(setEngagement).mockImplementationOnce(() => new Promise(resolve => {
      finishSave = () => resolve({ ok: true })
    }))
    render(<DeviceSettings onClose={vi.fn()} />)
    const input = await screen.findByLabelText('Maximum agent turns in one chain')
    // Type only once the loaded value is in, or its arrival resets the draft mid-typing.
    await waitFor(() => expect(input).toHaveValue(20))
    await user.clear(input)
    await user.type(input, '32')
    expect(setEngagement).not.toHaveBeenCalled()
    await user.tab()
    await waitFor(() => expect(setEngagement).toHaveBeenCalledWith({ max_relay_turns: 32 }))
    expect(screen.getByLabelText('Maximum agent turns in one chain')).toBe(input)
    expect(input).toBeDisabled()
    await act(async () => finishSave())
    expect(input).not.toBeDisabled()
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
  it('turns agents on and off with one switch, which installs when needed', async () => {
    const { getAgentPlugins, installAgentPlugin, removeAgent } = await import('../../api')
    const plugin = (agent: string, extra: object) => ({
      id: agent, agent, version: '', driver: 'acp', kind: 'native', requires_node: false, install_url: '',
      package: '', about: '', source: '', state: 'ready', configured: false, legacy_configured: false,
      executable: '', node_runtime_installed: true, node_runtime_version: null,
      runtime_program: agent, runtime_installed: true, runtime_login_command: null, ...extra,
    })
    vi.mocked(getAgentPlugins).mockResolvedValue({ plugins: [
      plugin('hermes', {}), plugin('codex', { configured: true }), plugin('openclaw', { runtime_installed: false }),
    ] } as never)
    render(<DeviceSettings onClose={vi.fn()} />)
    await userEvent.click(screen.getByRole('tab', { name: 'Agents' }))
    const hermes = await screen.findByRole('switch', { name: 'Enable @hermes' })
    expect(hermes).toHaveAttribute('aria-checked', 'false')
    await userEvent.click(hermes)
    expect(installAgentPlugin).toHaveBeenCalledWith('hermes', {})
    const codex = screen.getByRole('switch', { name: 'Enable @codex' })
    expect(codex).toHaveAttribute('aria-checked', 'true')
    await userEvent.click(codex)
    expect(removeAgent).toHaveBeenCalledWith('codex')
    expect(screen.getByRole('switch', { name: 'Enable @openclaw' })).toBeDisabled()
    expect(screen.queryByRole('button', { name: /ENABLE|INSTALL|CHECK AGAIN/ })).toBeNull()
  })
  it('lists registry agents apart, adds one under a chosen handle, and uninstalls it when switched off', async () => {
    const { getAgentPlugins, installAgentPlugin, uninstallAgentPlugin, removeAgent } = await import('../../api')
    const plugin = (id: string, extra: object) => ({
      id, agent: id, version: '1.0.0', driver: 'acp', kind: 'npm', requires_node: true, install_url: '',
      package: id, about: `${id} agent`, source: '', state: 'missing', configured: false, legacy_configured: false,
      executable: '', node_runtime_installed: true, node_runtime_version: 'v24.0.0',
      runtime_program: null, runtime_installed: null, runtime_login_command: null,
      origin: 'registry', registry: { name: id.toUpperCase(), license: 'MIT' }, ...extra,
    })
    vi.mocked(getAgentPlugins).mockResolvedValue({ plugins: [
      plugin('gemini', {}),
      plugin('goose', { kind: 'binary', package: '', verified: false }),
      plugin('cline', { state: 'ready', configured: true, update_version: '2.0.0' }),
      plugin('kilo', { kind: 'binary', state: 'ready', configured: true, update_version: '3.0.0', update_verified: false }),
      plugin('vibe', {}),
    ] } as never)
    vi.mocked(getAgentConfigFor).mockResolvedValue({
      ...CONFIG, agents: [{ name: 'vibe', driver: 'acp', command: ['/usr/local/bin/vibe'], status: 'ready' }],
    } as never)
    render(<DeviceSettings onClose={vi.fn()} />)
    await userEvent.click(screen.getByRole('tab', { name: 'Agents' }))

    // An installed registry agent sits with the others, offers its update,
    // and switching it off uninstalls it rather than only removing the handle.
    const cline = await screen.findByRole('switch', { name: 'Enable @cline' })
    expect(screen.queryByRole('switch', { name: 'Enable @gemini' })).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'UPDATE v2.0.0' }))
    expect(installAgentPlugin).toHaveBeenCalledWith('cline', { upgrade: true })
    // An update with no published checksum asks before installing.
    await userEvent.click(screen.getByRole('button', { name: 'UPDATE v3.0.0' }))
    expect(installAgentPlugin).not.toHaveBeenCalledWith('kilo', expect.anything())
    expect(screen.getByRole('alert')).toHaveTextContent('no checksum')
    await userEvent.click(screen.getByRole('button', { name: 'Update unverified' }))
    expect(installAgentPlugin).toHaveBeenCalledWith('kilo', { upgrade: true, allow_unverified: true })
    await userEvent.click(cline)
    expect(uninstallAgentPlugin).toHaveBeenCalledWith('cline')
    expect(removeAgent).not.toHaveBeenCalledWith('cline')

    // The rest are browsed in the registry section.
    await userEvent.click(screen.getByText(/ACP REGISTRY/))
    await userEvent.type(screen.getByLabelText('Search the ACP Registry'), 'gem')
    expect(screen.queryByRole('button', { name: 'Add GOOSE' })).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'Add GEMINI' }))
    const handle = screen.getByLabelText('Handle')
    await userEvent.clear(handle)
    await userEvent.type(handle, 'cline')
    expect(screen.getByRole('alert')).toHaveTextContent('@cline is already in use')
    await userEvent.clear(handle)
    await userEvent.type(handle, 'gem')
    await userEvent.click(screen.getByRole('button', { name: 'Add agent' }))
    expect(installAgentPlugin).toHaveBeenCalledWith('gemini', { handle: 'gem' })

    // A custom agent's handle is taken too, even though no plugin owns it.
    await userEvent.clear(screen.getByLabelText('Search the ACP Registry'))
    await userEvent.type(screen.getByLabelText('Search the ACP Registry'), 'vibe')
    await userEvent.click(screen.getByRole('button', { name: 'Add VIBE' }))
    expect(screen.getByRole('alert')).toHaveTextContent('@vibe is already in use')
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))

    // An unverified release says so and is only added with explicit consent.
    await userEvent.clear(screen.getByLabelText('Search the ACP Registry'))
    await userEvent.click(screen.getByRole('button', { name: 'Add GOOSE' }))
    expect(screen.getByRole('alert')).toHaveTextContent('no checksum')
    await userEvent.click(screen.getByRole('button', { name: 'Add unverified' }))
    expect(installAgentPlugin).toHaveBeenCalledWith('goose', { allow_unverified: true })
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
  it('opens global Behaviour from a Circle without showing global model controls in the Circle', async () => {
    render(<DeviceSettings circleId="c1" onClose={vi.fn()} />)
    await screen.findByText('Edit global Behaviour settings →')
    expect(screen.queryByText('How Warden decides')).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'Edit global Behaviour settings →' }))
    expect(await screen.findByText('How Warden decides')).toBeInTheDocument()
    expect(getAgentConfigFor).toHaveBeenLastCalledWith(null)
    await userEvent.click(screen.getByRole('button', { name: 'Close' }))
    await waitFor(() => expect(getAgentConfigFor).toHaveBeenLastCalledWith('c1'))
  })

  it('keeps Warden inline under Behaviour and protects unsaved policy edits', async () => {
    const close = vi.fn()
    render(<DeviceSettings onClose={close} />)
    const summary = await screen.findByText('How Warden decides')
    expect(screen.queryByRole('tab', { name: 'Warden' })).toBeNull()
    expect(summary.closest('details')).not.toHaveAttribute('open')
    await userEvent.click(summary)
    expect(screen.getByRole('tab', { name: 'Behaviour' })).toHaveAttribute('aria-selected', 'true')
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
    expect(await screen.findByRole('dialog', { name: 'Global settings' })).toBeTruthy()
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
