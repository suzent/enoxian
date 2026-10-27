import { describe, expect, it, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react'

const plugin = (agent: string, extra: Record<string, unknown> = {}) => ({
  id: `${agent}-acp`, agent, version: '', driver: 'acp', kind: 'native', requires_node: false, install_url: `https://example.test/${agent}`,
  package: '', about: '', source: '', state: 'ready', configured: false, legacy_configured: false, executable: '',
  node_runtime_installed: true, node_runtime_version: null, runtime_program: agent, runtime_installed: true, runtime_login_command: null,
  ...extra,
})

let enabled: string[] = []
const getAgentPlugins = vi.fn(async () => ({
  plugins: [
    plugin('claude', { configured: enabled.includes('claude') }),
    plugin('gemini', { runtime_installed: false }),
  ],
}))
const discoverAgents = vi.fn(async () => ({ agents: [] }))
const getAgentConfig = vi.fn(async () => ({ agents: enabled.map(name => ({ name })) }))
const installAgentPlugin = vi.fn(async () => { enabled = ['claude'] })
const addAgent = vi.fn()
const getAgentAuth = vi.fn(async () => ({}))
const kinds = (hint: string | null) => [
  { id: 'builtin:rules', name: 'Built-in rules', description: 'No model.', protocol: 'rules', needs_key: false, builtin: true, fields: {} },
  { id: 'jev', name: 'TypeSafe Jev', description: 'Hosted.', protocol: 'systemone', endpoint: 'https://api.typesafe.ai/v1/systemone',
    needs_key: true, builtin: true, fields: {}, key_hint: hint, key_url: 'https://docs.typesafe.ai/models' },
]
const getWarden = vi.fn()
const setWardenConfig = vi.fn(async () => ({ ok: true }))
const setWardenKey = vi.fn(async () => ({ key_hint: '••••cret' }))
const setEngagement = vi.fn(async () => ({}))
const testWarden = vi.fn()
vi.mock('../../api', () => ({
  getAgentPlugins, discoverAgents, getAgentConfig, installAgentPlugin, addAgent,
  getAgentAuth, startAgentAuth: vi.fn(), cancelAgentAuth: vi.fn(),
  getWarden, setWardenConfig, setWardenKey, setEngagement, testWarden,
}))

const { default: OnboardingSetup } = await import('../OnboardingSetup')

describe('OnboardingSetup', () => {
  beforeEach(() => { cleanup(); vi.clearAllMocks(); enabled = [] })

  it('enables a found agent, takes a pasted key, and turns the Warden on wide open', async () => {
    getWarden.mockResolvedValueOnce({ config: { decider: 'builtin:rules' }, kinds: kinds(null) })
      .mockResolvedValue({ config: { decider: 'builtin:rules' }, kinds: kinds('••••cret') })
    const done = vi.fn()
    render(<OnboardingSetup circleId="c1" onDone={done} />)

    expect(await screen.findByText('gemini not found')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'ENABLE' }))
    await waitFor(() => expect(installAgentPlugin).toHaveBeenCalledWith('claude-acp'))
    fireEvent.click(await screen.findByRole('button', { name: 'CONTINUE WITH 1 AGENT' }))

    // The recommended decider is chosen, but nothing reads a key for it.
    const jev = await screen.findByRole('radio', { name: /TypeSafe Jev/ })
    expect(jev.getAttribute('aria-checked')).toBe('true')
    expect(screen.getByText(/Until a key is saved, the built-in rules decide/)).toBeTruthy()
    fireEvent.change(screen.getByLabelText('API key'), { target: { value: 'sk-secret' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save key' }))
    await waitFor(() => expect(setWardenKey).toHaveBeenCalledWith('jev', 'sk-secret'))
    await waitFor(() => expect(screen.queryByText(/Until a key is saved/)).toBeNull())

    fireEvent.click(screen.getByRole('button', { name: 'TURN ON WARDEN' }))
    await waitFor(() => expect(done).toHaveBeenCalled())
    expect(setWardenConfig).toHaveBeenCalledWith({ decider: 'jev' })
    expect(setEngagement).toHaveBeenCalledWith({
      reaction: 'push', engagement_window_secs: 180, warden_mode: 'act', ambient_responders: 32, ambient: ['claude'],
      recommended_seen: 1,
    })
  })

  it('leaves everything as it was when skipped, and does not ask again straight after', async () => {
    const done = vi.fn()
    render(<OnboardingSetup onDone={done} />)
    fireEvent.click(await screen.findByRole('button', { name: 'SKIP SETUP' }))
    await waitFor(() => expect(done).toHaveBeenCalled())
    expect(setEngagement).toHaveBeenCalledWith({ recommended_seen: 1 })
    expect(setWardenConfig).not.toHaveBeenCalled()
  })
})
