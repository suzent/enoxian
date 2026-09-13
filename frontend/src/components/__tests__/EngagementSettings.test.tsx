/**
 * Engagement settings at two scopes.
 *
 * The thing worth testing is the scoping: an inherited setting must be
 * visibly inherited, and editing it must write to the scope you are looking
 * at — changing the wrong scope is the failure this UI exists to prevent.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import EngagementSettings from '../EngagementSettings'
import type { CircleSettingsView, SettingsView } from '../../types'

const GLOBAL: SettingsView = {
  reaction: 'push',
  engagement_window_secs: 180,
  ambient: [],
  max_relay_turns: 20,
}

function circle(over: CircleSettingsView['overrides'] = {}): CircleSettingsView {
  return {
    circle_id: 'c1',
    overrides: over,
    effective: { ...GLOBAL, ...over },
  }
}

let confirmSpy: ReturnType<typeof vi.spyOn>
beforeEach(() => { confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true) })
afterEach(() => { confirmSpy.mockRestore() })

describe('global scope', () => {
  it('shows no inherit badges — there is nothing above it', () => {
    render(
      <EngagementSettings agentNames={['claude']} global={GLOBAL} circle={null}
        scope="global" busy={false} onChange={vi.fn()} />,
    )
    expect(screen.queryByText('Using default')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Use default' })).toBeNull()
  })

  it('edits write to the global scope', async () => {
    const onChange = vi.fn()
    render(
      <EngagementSettings agentNames={['claude']} global={GLOBAL} circle={null}
        scope="global" busy={false} onChange={onChange} />,
    )
    // One change event: the input is controlled and this parent does not feed
    // the new value back, so typing would assert on intermediate states.
    fireEvent.change(screen.getByLabelText('Maximum agent turns in one chain'), {
      target: { value: '6' },
    })
    expect(onChange).toHaveBeenLastCalledWith({ max_relay_turns: 6 })
  })
})

describe('circle scope', () => {
  it('marks a setting the Circle does not override, while still showing its value', () => {
    render(
      <EngagementSettings agentNames={['claude']} global={GLOBAL} circle={circle()}
        scope="circle" busy={false} onChange={vi.fn()} />,
    )
    // Four settings, all inherited.
    expect(screen.getAllByText('Using default')).toHaveLength(4)
    // And the inherited value is visible, not hidden.
    expect(screen.getByLabelText('Follow-up window in seconds')).toHaveValue(180)
  })

  it('offers a reset only for settings this Circle actually overrides', () => {
    render(
      <EngagementSettings agentNames={['claude']} global={GLOBAL}
        circle={circle({ engagement_window_secs: 0 })}
        scope="circle" busy={false} onChange={vi.fn()} />,
    )
    expect(screen.getAllByRole('button', { name: 'Use default' })).toHaveLength(1)
    expect(screen.getAllByText('Using default')).toHaveLength(3)
  })

  it('reset clears the override rather than writing a value', async () => {
    // null is "inherit again"; 0 would be "override with off". Collapsing them
    // would quietly pin the Circle to whatever it happened to show.
    const onChange = vi.fn()
    render(
      <EngagementSettings agentNames={['claude']} global={GLOBAL}
        circle={circle({ engagement_window_secs: 0 })}
        scope="circle" busy={false} onChange={onChange} />,
    )
    await userEvent.click(screen.getByRole('button', { name: 'Use default' }))
    expect(onChange).toHaveBeenCalledWith({ engagement_window_secs: null })
  })

  it('shows the Circle value where it differs from global', () => {
    render(
      <EngagementSettings agentNames={['claude']} global={GLOBAL}
        circle={circle({ engagement_window_secs: 0 })}
        scope="circle" busy={false} onChange={vi.fn()} />,
    )
    expect(screen.getByLabelText('Follow-up window in seconds')).toHaveValue(0)
    expect(screen.getByText(/every message needs an explicit @mention/)).toBeTruthy()
  })
})

describe('reading the room', () => {
  it('lists one switch per configured agent, off by default', () => {
    render(
      <EngagementSettings agentNames={['claude', 'codex']} global={GLOBAL} circle={null}
        scope="global" busy={false} onChange={vi.fn()} />,
    )
    expect(screen.getByRole('checkbox', { name: '@claude' })).not.toBeChecked()
    expect(screen.getByRole('checkbox', { name: '@codex' })).not.toBeChecked()
  })

  it('asks before sending every message to a provider', async () => {
    const onChange = vi.fn()
    render(
      <EngagementSettings agentNames={['claude']} global={GLOBAL} circle={null}
        scope="global" busy={false} onChange={onChange} />,
    )
    await userEvent.click(screen.getByRole('checkbox', { name: '@claude' }))
    expect(confirmSpy.mock.calls[0][0]).toMatch(/sent to this agent's model provider/)
    expect(onChange).toHaveBeenCalledWith({ ambient: ['claude'] })
  })

  it('changes nothing when that question is declined', async () => {
    confirmSpy.mockReturnValue(false)
    const onChange = vi.fn()
    render(
      <EngagementSettings agentNames={['claude']} global={GLOBAL} circle={null}
        scope="global" busy={false} onChange={onChange} />,
    )
    await userEvent.click(screen.getByRole('checkbox', { name: '@claude' }))
    expect(onChange).not.toHaveBeenCalled()
  })

  it('turning one off keeps the others, and does not ask', async () => {
    const onChange = vi.fn()
    render(
      <EngagementSettings agentNames={['claude', 'codex']}
        global={{ ...GLOBAL, ambient: ['claude', 'codex'] }} circle={null}
        scope="global" busy={false} onChange={onChange} />,
    )
    await userEvent.click(screen.getByRole('checkbox', { name: '@claude' }))
    expect(confirmSpy).not.toHaveBeenCalled()
    expect(onChange).toHaveBeenCalledWith({ ambient: ['codex'] })
  })
})

describe('hand-offs', () => {
  it('has no opt-in switch — only a limit on how far a chain runs', () => {
    render(
      <EngagementSettings agentNames={['claude']} global={GLOBAL} circle={null}
        scope="global" busy={false} onChange={vi.fn()} />,
    )
    expect(screen.queryByText(/accepts hand-offs/i)).toBeNull()
    expect(screen.getByLabelText('Maximum agent turns in one chain')).toHaveValue(20)
    expect(screen.getByText(/Agents may hand work to each other/)).toBeTruthy()
  })
})

it('is inert while a save is in flight', async () => {
  const onChange = vi.fn()
  render(
    <EngagementSettings agentNames={['claude']} global={GLOBAL} circle={null}
      scope="global" busy onChange={onChange} />,
  )
  await userEvent.click(screen.getByRole('checkbox', { name: '@claude' }))
  expect(onChange).not.toHaveBeenCalled()
})

