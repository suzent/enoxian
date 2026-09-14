import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render, screen, waitFor, cleanup } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
const getExecutions = vi.fn()
const updateExecution = vi.fn(async () => ({}))
vi.mock('../../api', () => ({ getExecutions, updateExecution }))
const { default: ExecutionStatus } = await import('../ExecutionStatus')
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
})
