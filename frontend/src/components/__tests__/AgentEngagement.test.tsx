/**
 * Per-agent engagement switches.
 *
 * Both settings change what leaves this machine, so the tests are mostly about
 * the guard rails: off by default, and the expensive one asks first.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import AgentEngagement from '../AgentEngagement'
import type { AgentSummary } from '../../types'

function agent(over: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name: 'claude',
    driver: 'acp',
    command: ['claude-agent-acp'],
    working_dir: null,
    installed: true,
    status: 'ready',
    engagement: 'mention',
    accept_from: 'humans',
    max_relay_turns: 20,
    ...over,
  }
}

let confirmSpy: ReturnType<typeof vi.spyOn>

beforeEach(() => {
  confirmSpy = vi.spyOn(window, 'confirm')
})
afterEach(() => {
  confirmSpy.mockRestore()
})

describe('engagement switches', () => {
  it('shows both off for a default agent', () => {
    render(<AgentEngagement agent={agent()} busy={false} onChange={vi.fn()} />)
    expect(screen.getByRole('checkbox', { name: /reads the room/ })).not.toBeChecked()
    expect(screen.getByRole('checkbox', { name: /accepts hand-offs/ })).not.toBeChecked()
  })

  it('reflects what is already on', () => {
    render(
      <AgentEngagement
        agent={agent({ engagement: 'ambient', accept_from: 'agents' })}
        busy={false}
        onChange={vi.fn()}
      />,
    )
    expect(screen.getByRole('checkbox', { name: /reads the room/ })).toBeChecked()
    expect(screen.getByRole('checkbox', { name: /accepts hand-offs/ })).toBeChecked()
  })

  it('asks before letting an agent read the room', async () => {
    // This sends every message in the Circle to a model provider. Saying so
    // afterwards is too late.
    confirmSpy.mockReturnValue(true)
    const onChange = vi.fn()
    render(<AgentEngagement agent={agent()} busy={false} onChange={onChange} />)

    await userEvent.click(screen.getByRole('checkbox', { name: /reads the room/ }))
    expect(confirmSpy).toHaveBeenCalled()
    expect(confirmSpy.mock.calls[0][0]).toMatch(/sent to this agent's model provider/)
    expect(onChange).toHaveBeenCalledWith({ engagement: 'ambient' })
  })

  it('changes nothing when that question is declined', async () => {
    confirmSpy.mockReturnValue(false)
    const onChange = vi.fn()
    render(<AgentEngagement agent={agent()} busy={false} onChange={onChange} />)

    await userEvent.click(screen.getByRole('checkbox', { name: /reads the room/ }))
    expect(onChange).not.toHaveBeenCalled()
  })

  it('turning it back off does not ask — only arming is sensitive', async () => {
    const onChange = vi.fn()
    render(
      <AgentEngagement agent={agent({ engagement: 'ambient' })} busy={false} onChange={onChange} />,
    )
    await userEvent.click(screen.getByRole('checkbox', { name: /reads the room/ }))
    expect(confirmSpy).not.toHaveBeenCalled()
    expect(onChange).toHaveBeenCalledWith({ engagement: 'mention' })
  })

  it('hand-offs toggle without a prompt, and say what the cap is', async () => {
    const onChange = vi.fn()
    render(<AgentEngagement agent={agent({ max_relay_turns: 6 })} busy={false} onChange={onChange} />)

    const box = screen.getByRole('checkbox', { name: /accepts hand-offs/ })
    expect(box.closest('label')).toHaveAttribute('title', expect.stringContaining('6 turns'))
    await userEvent.click(box)
    expect(onChange).toHaveBeenCalledWith({ accept_from: 'agents' })
  })

  it('is inert while a save is in flight', async () => {
    const onChange = vi.fn()
    render(<AgentEngagement agent={agent()} busy onChange={onChange} />)
    await userEvent.click(screen.getByRole('checkbox', { name: /reads the room/ }))
    expect(onChange).not.toHaveBeenCalled()
  })
})
