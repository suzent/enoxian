import { describe, expect, it, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'

const getAgentCards = vi.fn()
vi.mock('../../api', () => ({ getAgentCards }))

const { default: AgentCards } = await import('../AgentCards')

const card = (over: Partial<Record<string, unknown>>) => ({
  handle: '@suzy/jessair/claude', owner: 'suzy', device: 'jessair', agent: 'claude',
  path: 'agents/suzy/jessair/claude.md', local: true, exists: false, summary: '', ...over,
})

describe('AgentCards', () => {
  beforeEach(() => { getAgentCards.mockReset() })

  it('opens cards that exist and says when one is still to be written', async () => {
    getAgentCards.mockResolvedValue({ cards: [
      card({}),
      card({ handle: '@bob/spark/codex', owner: 'bob', device: 'spark', agent: 'codex', local: false,
        path: 'agents/bob/spark/codex.md', exists: true, summary: 'Codex 0.40, GPT-5.5' }),
    ] })
    const open = vi.fn()
    render(<AgentCards circleId="c1" onOpenFile={open} />)

    expect(await screen.findByText('Codex 0.40, GPT-5.5')).toBeTruthy()
    expect(screen.getByText(/this device writes it shortly/)).toBeTruthy()
    // Cards are written by their device, never from here.
    expect(screen.queryByRole('button', { name: 'CREATE CARD' })).toBeNull()
    const buttons = screen.getAllByRole('button', { name: 'OPEN' })
    expect(buttons).toHaveLength(1)
    fireEvent.click(buttons[0])
    expect(open).toHaveBeenCalledWith('agents/bob/spark/codex.md')
  })
})
