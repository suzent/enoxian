import { describe, expect, it, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react'

const getWarden = vi.fn()
const setWardenConfig = vi.fn(async () => ({ ok: true }))
const setWardenKey = vi.fn(async () => ({ key_hint: '••••abcd' }))
const setWardenSpec = vi.fn(async () => ({ spec: {} }))
const resetWardenSpec = vi.fn()
const testWarden = vi.fn()
vi.mock('../../api', () => ({ getWarden, setWardenConfig, setWardenKey, setWardenSpec, resetWardenSpec, testWarden }))

const { default: WardenSettings, describeDecision } = await import('../WardenSettings')

const view = (decider: string, keyHint: string | null = null) => ({
  mode: 'off',
  config: { decider, timeout_secs: 3 },
  problem: decider === 'builtin:rules' || keyHint ? null : 'no API key is set',
  kinds: [
    { id: 'builtin:rules', name: 'Built-in rules', description: 'No model.', protocol: 'rules', needs_key: false, builtin: true, fields: {} },
    { id: 'litellm-systemone', name: 'LiteLLM (System One)', description: 'A proxy.', protocol: 'systemone',
      endpoint: 'http://localhost:4000/v1/systemone', model: 'jev', needs_key: true, builtin: true, fields: {}, key_hint: keyHint },
  ],
  spec_toml: 'version = "builtin-1"\n',
  spec: { version: 'builtin-1', modes: ['single', 'vote'], admit_threshold: 0.5, lambda: 0.05 },
  spec_source: { kind: 'default' },
})

const decision = {
  choice: { admit: true, mode: 'vote', listeners: 2, max_listeners: 3, agents: ['claude', 'codex'] },
  decider: 'litellm-systemone', spec_version: 'builtin-1', p_admit: 0.91,
  p_success: { single: 0.5, vote: 0.8 }, scores: { single: 0.45, vote: 0.68 },
  reason: 'vote p=0.80', latency_ms: 120, cost_usd: 0.0004,
}

describe('WardenSettings', () => {
  beforeEach(() => { cleanup(); vi.clearAllMocks() })

  it('chooses a decider, and takes a key it never shows again', async () => {
    getWarden.mockResolvedValueOnce(view('builtin:rules')).mockResolvedValue(view('litellm-systemone'))
    render(<WardenSettings />)
    fireEvent.change(await screen.findByRole('combobox', { name: 'Decider' }), { target: { value: 'litellm-systemone' } })
    await waitFor(() => expect(setWardenConfig).toHaveBeenCalledWith({ decider: 'litellm-systemone' }))
    expect(await screen.findByRole('alert')).toBeTruthy()
    expect(screen.getByText(/no API key is set/)).toBeTruthy()

    const input = await screen.findByLabelText('API key') as HTMLInputElement
    expect(input.type).toBe('password')
    fireEvent.change(input, { target: { value: 'sk-secret' } })
    fireEvent.click(screen.getByRole('button', { name: 'SET' }))
    await waitFor(() => expect(setWardenKey).toHaveBeenCalledWith('litellm-systemone', 'sk-secret'))
    await waitFor(() => expect((screen.getByLabelText('API key') as HTMLInputElement).value).toBe(''))
  })

  it('tries a sample message and shows what the warden would do', async () => {
    getWarden.mockResolvedValue(view('litellm-systemone', '••••abcd'))
    testWarden.mockResolvedValue({ decision, questions: ['admit', 'agent', 'success:single', 'success:vote'], input: '' })
    render(<WardenSettings circleId="c1" />)
    fireEvent.change(await screen.findByLabelText('Sample message'), { target: { value: 'Is this true?' } })
    fireEvent.click(screen.getByRole('button', { name: 'ASK THE WARDEN' }))
    await waitFor(() => expect(testWarden).toHaveBeenCalledWith('Is this true?', 'c1', false))
    expect(await screen.findByText(/blind vote, 2–3 voters/)).toBeTruthy()
    expect(screen.getByText(/Needs an answer: 91%/)).toBeTruthy()
  })

  it('lets agents\' own messages reach the warden', async () => {
    getWarden.mockResolvedValue(view('builtin:rules'))
    render(<WardenSettings />)
    fireEvent.click(await screen.findByRole('checkbox', { name: "Agents' messages too" }))
    await waitFor(() => expect(setWardenConfig).toHaveBeenCalledWith({ agent_messages: true }))
  })

  it('warns that agents\' messages need a decider', async () => {
    getWarden.mockResolvedValue({ ...view('builtin:rules'), config: { decider: 'builtin:rules', timeout_secs: 5, agent_messages: true } })
    render(<WardenSettings />)
    expect(await screen.findByText(/need a decider to judge them/)).toBeTruthy()
  })

  it('applies an edited spec', async () => {
    getWarden.mockResolvedValue(view('builtin:rules'))
    render(<WardenSettings />)
    const spec = await screen.findByLabelText('Warden spec')
    fireEvent.change(spec, { target: { value: 'version = "mine"\n' } })
    fireEvent.click(screen.getByRole('button', { name: 'APPLY' }))
    await waitFor(() => expect(setWardenSpec).toHaveBeenCalledWith('version = "mine"\n'))
  })
})

describe('describeDecision', () => {
  it('says who answers, or that nobody does', () => {
    expect(describeDecision({ ...decision, choice: { ...decision.choice, mode: 'single', listeners: 1, max_listeners: 1 } } as never))
      .toBe('one listener (claude) — vote p=0.80')
    expect(describeDecision({ ...decision, reason: 'no reply needed', choice: { ...decision.choice, admit: false, mode: null } } as never))
      .toBe('nobody answers — no reply needed')
  })
})
