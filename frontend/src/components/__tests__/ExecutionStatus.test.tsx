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
describe('delivery controls', () => {
  it('shows remote outcomes while keeping retry local and explicit', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    render(<ExecutionStatus circleId="circle" />)
    await userEvent.click(await screen.findByText(/Agent delivery/))
    expect(await screen.findByText('interrupted')).toBeTruthy()
    expect(await screen.findByText('failed')).toBeTruthy()
    expect(screen.getAllByText('Retry')).toHaveLength(1)
    await userEvent.click(screen.getByText('Retry'))
    await waitFor(() => expect(updateExecution).toHaveBeenCalledWith('circle', 'r1', 'retry'))
  })
  it('does not retry when the duplicate-effects confirmation is declined', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false)
    render(<ExecutionStatus circleId="circle" />)
    await userEvent.click(await screen.findByText(/Agent delivery/))
    await userEvent.click(await screen.findByText('Retry'))
    expect(updateExecution).not.toHaveBeenCalled()
  })
})
