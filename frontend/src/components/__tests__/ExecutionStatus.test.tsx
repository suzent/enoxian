import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render, screen, waitFor, cleanup } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
const getExecutions = vi.fn()
const updateExecution = vi.fn(async () => ({}))
vi.mock('../../api', () => ({ getExecutions, updateExecution }))
const { default: ExecutionStatus, describeActivity, QUIET_HINT_SECS } = await import('../ExecutionStatus')
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
    await userEvent.click(await screen.findByText(/Needs attention/))
    expect(screen.getByText('Stopped unexpectedly')).toBeTruthy()
    expect(screen.getByText('macbook-pro · this device')).toBeTruthy()
    expect(screen.getByText('Retry on jessair.')).toBeTruthy()
    expect(screen.getAllByRole('button', { name: 'Try again' })).toHaveLength(1)
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(updateExecution).toHaveBeenCalledWith('circle', 'r1', 'retry'))
  })
  it('does not retry when confirmation is declined', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false)
    render(<ExecutionStatus circleId="circle" />)
    await userEvent.click(await screen.findByText(/Needs attention/))
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(updateExecution).not.toHaveBeenCalled()
  })
  it('keeps imported markers out of active counts and hides their retry controls', async () => {
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'old', message_id: 'm1', agent_id: '~ambient:claude', peer_id: 'local', status: 'legacy_suppressed', ambient: false },
    ] })
    render(<ExecutionStatus circleId="circle" />)
    await waitFor(() => expect(getExecutions).toHaveBeenCalled())
    expect(screen.getByText('No active requests')).toBeTruthy()
    expect(screen.queryByText(/Before this update/)).toBeNull()
    expect(screen.queryByText('@claude')).toBeNull()
    expect(screen.queryByRole('button', { name: /Try again/ })).toBeNull()
  })
  it('does not ask you to retry a turn the device already put back', async () => {
    // A restart expires whatever was queued and the drain requeues it. Both
    // records exist, and listing the dead one under "Needs attention" with a
    // Try again button makes a successful recovery read as a failure.
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'dead', message_id: 'm1', agent_id: 'claude', peer_id: 'local', status: 'expired', ambient: true, detail: 'ambient observation expired on restart' },
      { run_id: 'back', message_id: 'm1', agent_id: 'claude', peer_id: 'local', status: 'running', ambient: true },
    ] })
    render(<ExecutionStatus circleId="circle" />)
    await waitFor(() => expect(getExecutions).toHaveBeenCalled())
    expect(screen.queryByText(/Needs attention/)).toBeNull()
    expect(screen.queryByRole('button', { name: /Try again/ })).toBeNull()
    // The superseded run is still findable, just not demanding action.
    await userEvent.click(await screen.findByText(/Recent history/))
    expect(screen.getByText(/expired on restart/)).toBeTruthy()
  })
  it('still flags a failure nothing has picked up', async () => {
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'dead', message_id: 'm1', agent_id: 'claude', peer_id: 'local', status: 'failed', ambient: true, detail: 'adapter exited' },
    ] })
    render(<ExecutionStatus circleId="circle" />)
    await waitFor(() => expect(getExecutions).toHaveBeenCalled())
    expect(await screen.findByText(/Needs attention/)).toBeTruthy()
    await userEvent.click(screen.getByText(/Needs attention/))
    expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy()
  })
  it('distinguishes a quiet listener from a waiting request', async () => {
    getExecutions.mockResolvedValue({ peer_id: 'local', runs: [
      { run_id: 'wait', message_id: 'm1', agent_id: 'codex', peer_id: 'local', status: 'pending', ambient: true },
      { run_id: 'pass', message_id: 'm2', agent_id: 'claude', peer_id: 'local', status: 'completed', ambient: true, detail: 'No reply needed' },
    ] })
    render(<ExecutionStatus circleId="circle" />)
    expect(await screen.findByText('0 working · 1 waiting')).toBeTruthy()
    expect(screen.getByText('Waiting for a turn')).toBeTruthy()
    await userEvent.click(screen.getByText(/Recent history/))
    expect(screen.getAllByText('No reply needed')).toHaveLength(1)
    expect(screen.getByRole('button', { name: 'Cancel request' })).toBeTruthy()
  })
  it('does not let an old Circle response replace the current one', async () => {
    let resolveOld!: (value: unknown) => void
    getExecutions.mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve }))
    const { rerender } = render(<ExecutionStatus circleId="old" />)
    rerender(<ExecutionStatus circleId="new" />)
    await screen.findByText(/Needs attention/)
    resolveOld({ peer_id: 'wrong', runs: [] })
    await waitFor(() => expect(screen.getByText(/Needs attention/)).toBeTruthy())
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
})
