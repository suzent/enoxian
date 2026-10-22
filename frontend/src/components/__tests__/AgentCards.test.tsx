import { describe, expect, it, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor } from '@testing-library/react'

const getAgentCards = vi.fn()
const createAgentCard = vi.fn()
vi.mock('../../api', () => ({ getAgentCards, createAgentCard }))

const { default: AgentCards } = await import('../AgentCards')

const card = (over: Partial<Record<string, unknown>>) => ({
  handle: '@suzy/jessair/claude', owner: 'suzy', device: 'jessair', agent: 'claude',
  path: 'agents/suzy/jessair/claude.md', local: true, exists: false, summary: '', ...over,
})

describe('AgentCards', () => {
  beforeEach(() => { getAgentCards.mockReset(); createAgentCard.mockReset() })

  it('offers a card only for this device\'s agents and opens what it wrote', async () => {
    getAgentCards.mockResolvedValue({ cards: [
      card({}),
      card({ handle: '@bob/spark/codex', owner: 'bob', device: 'spark', agent: 'codex', local: false,
        path: 'agents/bob/spark/codex.md', exists: true, summary: 'Runs the GPU tests' }),
    ] })
    createAgentCard.mockResolvedValue({ path: 'agents/suzy/jessair/claude.md', created: true })
    const open = vi.fn()
    render(<AgentCards circleId="c1" onOpenFile={open} />)

    expect(await screen.findByText('Runs the GPU tests')).toBeTruthy()
    // One create button: the remote agent's card is not ours to write.
    const create = screen.getAllByRole('button', { name: 'CREATE CARD' })
    expect(create).toHaveLength(1)
    fireEvent.click(create[0])
    await waitFor(() => expect(createAgentCard).toHaveBeenCalledWith('c1', 'claude'))
    await waitFor(() => expect(open).toHaveBeenCalledWith('agents/suzy/jessair/claude.md'))
  })
})
