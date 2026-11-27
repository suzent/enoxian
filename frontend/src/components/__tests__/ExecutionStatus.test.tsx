import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render, screen, waitFor, cleanup } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
const getExecutions = vi.fn()
const updateExecution = vi.fn(async () => ({}))
const getWardenDecisions = vi.fn(async () => ({ mode: 'off', records: [] as unknown[] }))
vi.mock('../../api', () => ({ getExecutions, updateExecution, getWardenDecisions, getAgentIcons: vi.fn(async () => ({ icons: {} })), agentIconUrl: (id: string) => `/api/agent-icons/${id}.svg` }))
const { default: ExecutionStatus, describeActivity, describeUsage, usageToday, QUIET_HINT_SECS } = await import('../ExecutionStatus')
beforeEach(() => { cleanup(); vi.clearAllMocks(); getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
  { run_id: 'r1', message_id: 'm1', agent_id: 'claude', peer_id: 'local', status: 'interrupted', ambient: false },
  { run_id: 'r2', message_id: 'm2', agent_id: 'codex', peer_id: 'remote', status: 'failed', ambient: false },
] }) })
describe('agent activity', () => {
  it('uses device names and keeps retry local and explicit', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    render(<ExecutionStatus circleId="circle" members={[
      { peer_id: 'local', owner: 'suzy', agent_id: 'suzy-local', device_label: 'macbook-pro', agents: ['claude'], role: 'admin' },
      { peer_id: 'remote', owner: 'suzy', agent_id: 'suzy-remote', device_label: 'jessair', agents: ['codex'], role: 'member' },
    ]} />)
    await screen.findByTitle(/can be tried again/)
    expect(screen.getByText('Interrupted')).toBeTruthy()
    expect(screen.getByText('macbook-pro (this one)')).toBeTruthy()
    expect(screen.getByText('Retry on jessair.')).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Try again' })).toHaveLength(1)
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(updateExecution).toHaveBeenCalledWith('circle', 'r1', 'retry'))
  })
  it('does not retry when confirmation is declined', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false)
    render(<ExecutionStatus circleId="circle" />)
    await screen.findByTitle(/can be tried again/)
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(updateExecution).not.toHaveBeenCalled()
  })
  it('keeps imported markers out of active counts and hides their retry controls', async () => {
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'old', message_id: 'm1', agent_id: '~ambient:claude', peer_id: 'local', status: 'legacy_suppressed', ambient: false },
    ] })
    render(<ExecutionStatus circleId="circle" />)
    await waitFor(() => expect(getExecutions).toHaveBeenCalled())
    expect(screen.getByText('Nothing running')).toBeTruthy()
    expect(screen.queryByText(/Before this update/)).toBeNull()
    expect(screen.queryByText('@claude')).toBeNull()
    expect(screen.queryByRole('button', { name: /Try again/ })).toBeNull()
  })
  it('does not ask you to retry a turn the device already put back', async () => {
    // A restart expires whatever was queued and the drain requeues it. Both
    // records exist, and listing the dead one under "Failed" with a
    // Try again button makes a successful recovery read as a failure.
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'dead', message_id: 'm1', agent_id: 'claude', peer_id: 'local', status: 'expired', ambient: true, detail: 'ambient observation expired on restart' },
      { run_id: 'back', message_id: 'm1', agent_id: 'claude', peer_id: 'local', status: 'running', ambient: true },
    ] })
    render(<ExecutionStatus circleId="circle" />)
    await waitFor(() => expect(getExecutions).toHaveBeenCalled())
    expect(screen.queryByTitle(/can be tried again/)).toBeNull()
    expect(screen.queryByRole('button', { name: /Try again/ })).toBeNull()
    // The superseded run is still findable, just not demanding action.
    await userEvent.click(await screen.findByText(/^Done/))
    expect(screen.getByText(/expired on restart/)).toBeTruthy()
  })
  it('still flags a failure nothing has picked up', async () => {
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'dead', message_id: 'm1', agent_id: 'claude', peer_id: 'local', status: 'failed', ambient: true, detail: 'adapter exited' },
    ] })
    render(<ExecutionStatus circleId="circle" />)
    await waitFor(() => expect(getExecutions).toHaveBeenCalled())
    // Open without a click: it is the one thing that needs someone.
    expect(await screen.findByTitle(/can be tried again/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy()
  })
  it('distinguishes a quiet listener from a waiting request', async () => {
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'wait', message_id: 'm1', agent_id: 'codex', peer_id: 'local', status: 'pending', ambient: true },
      { run_id: 'pass', message_id: 'm2', agent_id: 'claude', peer_id: 'local', status: 'completed', ambient: true, detail: 'No reply needed' },
    ] })
    render(<ExecutionStatus circleId="circle" />)
    expect((await screen.findByTitle(/Status is reported by each device/)).textContent).toContain('Working0Queued1')
    expect(screen.getByText('Queued', { selector: '.agent-activity__status' })).toBeTruthy()
    await userEvent.click(screen.getByText(/^Done/))
    expect(screen.getAllByText('No reply')).toHaveLength(1)
    expect(screen.queryByText('No reply needed')).toBeNull()
    expect(screen.getByRole('button', { name: 'Cancel request' })).toBeTruthy()
  })
  it('says once that an agent is broken instead of listing every message it failed', async () => {
    const expired = 'ACP request failed (code -32603): Internal error: Failed to authenticate: OAuth session expired'
    const now = Math.floor(Date.now() / 1000)
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'a', message_id: 'm1', agent_id: 'claude', peer_id: 'remote', status: 'failed', ambient: true, detail: expired, updated_at: now - 30 },
      { run_id: 'b', message_id: 'm2', agent_id: 'claude', peer_id: 'remote', status: 'failed', ambient: true, detail: expired, updated_at: now - 20 },
      { run_id: 'c', message_id: 'm3', agent_id: 'codex', peer_id: 'local', status: 'failed', ambient: false, detail: 'turn timed out', updated_at: now - 10 },
      // Two days old: filed under Done, not offered for a retry.
      { run_id: 'd', message_id: 'm4', agent_id: 'codex', peer_id: 'local', status: 'failed', ambient: false, detail: 'turn timed out', updated_at: now - 2 * 86_400 },
    ] })
    const onOpenMembers = vi.fn()
    render(<ExecutionStatus circleId="circle" onOpenMembers={onOpenMembers} members={[
      { peer_id: 'remote', owner: 'suzy', agent_id: 'suzy-remote', device_label: 'jessair', agents: ['claude'], role: 'member' },
    ]} />)
    expect(await screen.findByText(/⚠ sign-in/)).toBeTruthy()
    expect(screen.getAllByText('jessair').length).toBeGreaterThan(0)
    expect(screen.getByText(/2 missed/)).toBeTruthy()
    // Only the one-off failure is left to retry.
    expect(screen.getByTitle(/can be tried again/).textContent).toContain('1')
    await userEvent.click(screen.getByRole('button', { name: /Members →/ }))
    expect(onOpenMembers).toHaveBeenCalled()
  })
  it('opens settings at a local agent that needs signing in, and only a local one', async () => {
    const expired = 'Internal error: Failed to authenticate: OAuth session expired'
    const now = Math.floor(Date.now() / 1000)
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'a', message_id: 'm1', agent_id: 'claude', peer_id: 'local', status: 'failed', ambient: true, detail: expired, updated_at: now },
      { run_id: 'b', message_id: 'm2', agent_id: 'codex', peer_id: 'remote', status: 'failed', ambient: true, detail: expired, updated_at: now },
    ] })
    const opened = vi.fn()
    window.addEventListener('enoxian:open-agent-settings', e => opened((e as CustomEvent).detail))
    render(<ExecutionStatus circleId="circle" />)
    await userEvent.click(await screen.findByRole('button', { name: /Sign in/ }))
    expect(opened).toHaveBeenCalledWith('claude')
    // The remote one can only be fixed on its own device, so it says how.
    expect(screen.getAllByRole('button', { name: /Sign in →/ })).toHaveLength(1)
    await userEvent.click(screen.getByRole('button', { name: /sign-in/ }))
    expect(screen.getByRole('dialog', { name: 'How to fix @codex' }).textContent).toMatch(/Sign @codex in again on device remote/)
  })
  it('does not let an old Circle response replace the current one', async () => {
    let resolveOld!: (value: unknown) => void
    getExecutions.mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve }))
    const { rerender } = render(<ExecutionStatus circleId="old" />)
    rerender(<ExecutionStatus circleId="new" />)
    await screen.findByTitle(/can be tried again/)
    resolveOld({ peer_id: 'wrong', runs: [] })
    await waitFor(() => expect(screen.getByTitle(/can be tried again/)).toBeTruthy())
  })
  it('shows what a running turn is doing and stops it only after confirmation', async () => {
    const now = Date.now() / 1000
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'r9', message_id: 'm9', agent_id: 'claude', peer_id: 'local', status: 'running', ambient: false },
    ], activity: { r9: { started_at: now - 300, idle_secs: 200, stopping: false,
      open_tools: [{ title: 'cargo test', status: 'in_progress', since: now - 240 }] } } })
    const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(false).mockReturnValueOnce(true)
    render(<ExecutionStatus circleId="circle" />)
    expect(await screen.findByText(/using cargo test \(4m\)/)).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: 'Stop' }))
    expect(updateExecution).not.toHaveBeenCalled()
    await userEvent.click(screen.getByRole('button', { name: 'Stop' }))
    await waitFor(() => expect(updateExecution).toHaveBeenCalledWith('circle', 'r9', 'cancel'))
    expect(confirm).toHaveBeenCalledTimes(2)
  })
})

describe('describeActivity', () => {
  const base = { started_at: 0, open_tools: [], stopping: false }
  it('names an open tool call instead of calling the turn quiet', () => {
    const d = describeActivity({ ...base, idle_secs: QUIET_HINT_SECS * 3,
      open_tools: [{ title: 'build', status: 'in_progress', since: 0 }] }, 3600)
    expect(d.quiet).toBe(false)
    expect(d.text).toContain('using build')
  })
  it('hints, and only hints, when nothing is reported for a long time', () => {
    expect(describeActivity({ ...base, idle_secs: QUIET_HINT_SECS - 1 }, 1000).quiet).toBe(false)
    const quiet = describeActivity({ ...base, idle_secs: QUIET_HINT_SECS }, 1000)
    expect(quiet.quiet).toBe(true)
    expect(quiet.text).toContain('may be stuck')
  })
  it('says when a turn is stopping', () => {
    expect(describeActivity({ ...base, idle_secs: 0, stopping: true }, 5).text).toBe('Stopping…')
  })
  it('describes the latest ACP phase and file when no tool is open', () => {
    expect(describeActivity({ ...base, idle_secs: 2, last_kind: 'fs/read_text_file', last_file: 'src/main.rs' }, 20).text)
      .toBe('Running 20s · reading src/main.rs · 2s ago')
    expect(describeActivity({ ...base, idle_secs: 1, last_kind: 'agent_message_chunk' }, 15).text)
      .toBe('Running 15s · writing reply · 1s ago')
  })
  it('renders plan steps, completed tool calls, and context window meter', async () => {
    const now = Math.floor(Date.now() / 1000)
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'r10', message_id: 'm10', agent_id: 'claude', peer_id: 'local', status: 'running', ambient: false },
    ], activity: {
      r10: {
        started_at: now - 60,
        idle_secs: 3,
        last_kind: 'agent_message_chunk',
        open_tools: [],
        recent_tools: [
          { title: 'cargo check', kind: 'execute', status: 'completed', duration_secs: 12, finished_at: now - 10 },
        ],
        plan: [
          { content: 'Inspect ACP liveness', status: 'completed' },
          { content: 'Render richer status', status: 'in_progress' },
        ],
        stopping: false,
      },
    }, usage: {
      r10: { context_used: 170_000, context_size: 200_000, cost: { amount: 0.08, currency: 'USD' } },
    } })
    render(<ExecutionStatus circleId="circle" />)
    expect(await screen.findByText('Inspect ACP liveness')).toBeTruthy()
    expect(screen.getByText('Render richer status')).toBeTruthy()
    // The tool list is folded into a count until asked for.
    expect(screen.queryByText('cargo check')).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: /1 tool/ }))
    expect(screen.getByText('cargo check')).toBeTruthy()
    expect(screen.getByText('execute')).toBeTruthy()
    const meter = screen.getByRole('meter', { name: 'Context window usage' })
    expect(meter.getAttribute('aria-valuenow')).toBe('85')
  })
  it('shows what a turn on this device used, and nothing for another device', async () => {
    const now = Math.floor(Date.now() / 1000)
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'mine', message_id: 'm1', agent_id: 'claude', peer_id: 'local', status: 'completed', ambient: true, updated_at: now },
      { run_id: 'theirs', message_id: 'm2', agent_id: 'codex', peer_id: 'remote', status: 'completed', ambient: true, updated_at: now },
    ], usage: {
      mine: { context_used: 41_000, context_size: 200_000, cost: { amount: 0.03, currency: 'USD' } },
      theirs: { context_used: 1, context_size: 2, cost: { amount: 9, currency: 'USD' } },
    }, spend: [
      { run_id: 'mine', ambient: true, updated_at: now, usage: { context_used: 41_000, context_size: 200_000, cost: { amount: 0.03, currency: 'USD' } } },
    ] })
    render(<ExecutionStatus circleId="circle" />)
    await userEvent.click(await screen.findByText(/^Done/))
    expect(screen.getByText(/context 41k \/ 200k/)).toBeTruthy()
    expect(screen.getByTitle(/Today on this device: scrying .*0\.03.* \(1 turn\)/)).toBeTruthy()
    expect(screen.queryByText(/context 0k/)).toBeNull()
  })
})

describe('usage', () => {
  it('says what was reported and no more', () => {
    expect(describeUsage({ context_used: 41_000, context_size: 200_000, cost: { amount: 0.03, currency: 'USD' } })).toMatch(/0\.03.* · context 41k \/ 200k/)
    expect(describeUsage({ context_used: 900, context_size: 0 })).toBe('')
    expect(describeUsage({ context_used: 33_477, context_size: 1_000_000, cost: { amount: 0.335, currency: 'USD' },
      tokens: { input: 2, output: 4, cached_read: 0, cached_write: 33_471, total: 33_477 } })).toMatch(/0\.335.* · 33k tokens · context 33k \/ 1M$/)
  })
  it('totals today by kind and counts unpriced turns apart', () => {
    const now = new Date()
    const t = Math.floor(now.getTime() / 1000)
    const priced = (amount: number) => ({ context_used: 1, context_size: 2, cost: { amount, currency: 'USD' } })
    const spend = [
      { run_id: 'a', ambient: true, updated_at: t, usage: priced(0.1) },
      { run_id: 'b', ambient: true, updated_at: t, usage: priced(0.2) },
      { run_id: 'c', ambient: false, updated_at: t, usage: { context_used: 1, context_size: 2 } },
      { run_id: 'old', ambient: true, updated_at: t - 2 * 86_400, usage: priced(5) },
    ]
    expect(usageToday(spend, now)).toMatch(/^scrying .*0\.30.* \(2 turns\) · addressed no cost reported \(1 turn\) · 1 without a cost$/)
    expect(usageToday(spend.slice(3), now)).toBeUndefined()
  })
})
