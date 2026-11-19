import { describe, expect, it } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import RoutingStrip, { deriveRouting, isRoutingNotice } from '../RoutingStrip'
import type { ChatMessage, ExecutionRun } from '../../types'

const nameOf = (agent: string, peer?: string) => (peer ? `${agent} on ${peer}` : agent)

function run(over: Partial<ExecutionRun>): ExecutionRun {
  return {
    run_id: 'r1', message_id: 'q', agent_id: 'claude', peer_id: 'jessair',
    status: 'completed', ambient: true, admitted_at: 100, updated_at: 110, ...over,
  }
}

const warden = {
  routed_by: 'warden' as const,
  warden: { mode: 'single' as const, decider: 'builtin:rules', reason: 'a direct question', confidence: 0.82, latency_ms: 40 },
}

function notice(id: string, text: string): ChatMessage {
  return { id, agent_id: 'system', author: 'system', text, mentions: [], ts: 120, reply_to: 'q' }
}

describe('deriveRouting', () => {
  it('names who the warden chose, with its confidence, and stays a chip once answered', () => {
    const routing = deriveRouting([run(warden)], ['claude on jessair'], [], nameOf)!
    expect(routing.routedBy).toBe('warden')
    expect(routing.chosen).toEqual(['claude on jessair'])
    expect(routing.confidence).toBe(0.82)
    expect(routing.state).toBe('answered')
    expect(routing.handedOn).toBe(false)
    expect(routing.steps[0].text).toBe('Warden (builtin:rules) chose single → claude on jessair in 0.0s')
    expect(routing.steps[0].detail).toBe('a direct question')
    expect(routing.steps[routing.steps.length - 1].text).toBe('Answered by claude on jessair')
  })

  it('shows nothing for an addressed turn that went fine', () => {
    expect(deriveRouting([run({ ambient: false, routed_by: 'mention' })], ['claude on jessair'], [], nameOf)).toBeNull()
    expect(deriveRouting([run({ ambient: false, routed_by: 'mention', status: 'running' })], [], [], nameOf)).toBeNull()
  })

  it('reads a receipt from an older peer by whether it was ambient', () => {
    expect(deriveRouting([run({ status: 'running' })], [], [], nameOf)!.routedBy).toBe('rotation')
  })

  it('marks the listener tried after a failure as handed on, and drops the notice once it answers', () => {
    const runs = [
      run({ ...warden, run_id: 'a', status: 'interrupted', admitted_at: 100, updated_at: 130, detail: 'daemon restarted' }),
      run({ run_id: 'b', agent_id: 'codex', peer_id: 'mini', status: 'completed', admitted_at: 140, updated_at: 160 }),
    ]
    const unanswered = notice('n1', 'No agent could answer this — every agent reading this room tried and could not answer.')
    const routing = deriveRouting(runs, ['codex on mini'], [unanswered], nameOf)!
    expect(routing.handedOn).toBe(true)
    expect(routing.chosen).toEqual(['claude on jessair'])
    expect(routing.state).toBe('answered')
    const texts = routing.steps.map(s => s.text)
    expect(texts).toContain('claude on jessair: stopped when its daemon restarted')
    expect(texts).toContain('Handed on to codex on mini: answered')
    expect(texts.some(t => t.startsWith('No agent could answer'))).toBe(false)
  })

  it('folds the daemon notice in as the last step while nobody has answered', () => {
    const runs = [run({ ...warden, status: 'failed', detail: 'auth expired' })]
    const routing = deriveRouting(runs, [], [notice('n1', 'No agent could answer this — x.')], nameOf)!
    expect(routing.state).toBe('failed')
    expect(routing.steps[routing.steps.length - 1]).toMatchObject({ tone: 'failed', text: 'No agent could answer this — x.' })
    expect(routing.steps.find(s => s.key === 'r1')!.detail).toBe('auth expired')
  })

  it('says a completed turn with no reply passed', () => {
    expect(deriveRouting([run(warden)], [], [], nameOf)!.state).toBe('passed')
  })

  it('says the voters whose answer was not posted voted, not passed', () => {
    const vote = { routed_by: 'warden' as const, warden: { ...warden.warden, mode: 'vote' as const } }
    const runs = [
      run({ ...vote, run_id: 'a' }),
      run({ ...vote, run_id: 'b', agent_id: 'codex', peer_id: 'mini' }),
    ]
    const routing = deriveRouting(runs, ['claude on jessair'], [], nameOf)!
    expect(routing.state).toBe('answered')
    const texts = routing.steps.map(s => s.text)
    expect(texts).toContain('claude on jessair: answered')
    expect(texts).toContain('codex on mini: voted')
  })

  it('does not count a co-listener admitted with the failed run as handed on', () => {
    const runs = [
      run({ ...warden, run_id: 'a', status: 'failed', admitted_at: 100, updated_at: 100 }),
      run({ ...warden, run_id: 'b', agent_id: 'codex', status: 'running', admitted_at: 100 }),
    ]
    expect(deriveRouting(runs, [], [], nameOf)!.handedOn).toBe(false)
  })
})

describe('isRoutingNotice', () => {
  it('matches the daemon notices for a message, not other system posts', () => {
    expect(isRoutingNotice(notice('n', 'No agent could answer this — y.'))).toBe(true)
    expect(isRoutingNotice(notice('n', 'claude (on suzy/jessair) could not answer: timed out'))).toBe(true)
    expect(isRoutingNotice({ ...notice('n', 'No agent could answer this.'), reply_to: null })).toBe(false)
    expect(isRoutingNotice(notice('n', 'Relay stopped by suzy'))).toBe(false)
  })
})

describe('RoutingStrip', () => {
  it('is a chip while things go to plan, and opens on click', () => {
    const routing = deriveRouting([run(warden)], ['claude on jessair'], [], nameOf)!
    render(<RoutingStrip routing={routing} />)
    const chip = screen.getByRole('button', { expanded: false })
    expect(chip).toHaveTextContent('Warden→claude on jessair· one listener· 82%answered')
    expect(screen.queryByText('a direct question')).toBeNull()
    fireEvent.click(chip)
    expect(screen.getByText('a direct question')).toBeInTheDocument()
  })

  it('opens by itself when nobody could answer', () => {
    const routing = deriveRouting([run({ ...warden, status: 'failed' })], [], [], nameOf)!
    render(<RoutingStrip routing={routing} />)
    expect(screen.getByRole('button', { expanded: true })).toHaveTextContent('no answer')
    expect(screen.getByText('No answer yet')).toBeInTheDocument()
  })
})
