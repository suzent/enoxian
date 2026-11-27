import { describe, expect, it } from 'vitest'
import { brokenAgents, classifyFailure, healthKey } from '../agentHealth'
import type { ExecutionRun } from '../../types'

const run = (over: Partial<ExecutionRun>): ExecutionRun => ({
  run_id: Math.random().toString(36), message_id: 'm', agent_id: 'claude', peer_id: 'jess',
  status: 'failed', ambient: true, ...over,
} as ExecutionRun)

const EXPIRED = 'ACP prompt turn failed: ACP request failed (code -32603): Internal error: Failed to authenticate: OAuth session expired and could not be refreshed'

describe('classifyFailure', () => {
  it('tells an agent that needs signing in from a message that went wrong', () => {
    expect(classifyFailure(EXPIRED)).toBe('sign_in')
    expect(classifyFailure('ACP request failed (code -32000): Suzent authorization required.')).toBe('sign_in')
    expect(classifyFailure('failed to spawn ACP agent `nope`: No such file')).toBe('missing')
    expect(classifyFailure('turn timed out after 600s')).toBeNull()
    expect(classifyFailure('ACP request failed (code -32000): this authorization cannot access the requested session')).toBeNull()
    expect(classifyFailure(undefined)).toBeNull()
  })
})

describe('brokenAgents', () => {
  it('is broken while its latest finished turn failed that way, per device', () => {
    const broken = brokenAgents([
      run({ updated_at: 1, status: 'completed' }),
      run({ updated_at: 2, detail: EXPIRED }),
      run({ updated_at: 3, detail: EXPIRED }),
      run({ updated_at: 3, peer_id: 'mac', status: 'completed' }),
    ])
    const claude = broken.get(healthKey('jess', 'claude'))!
    expect(claude.problem).toBe('sign_in')
    expect(claude.failures).toBe(2)
    expect(claude.since).toBe(2)
    expect(broken.has(healthKey('mac', 'claude'))).toBe(false)
  })

  it('is well again once a turn succeeds, and ambient runs count as the same agent', () => {
    const broken = brokenAgents([
      run({ updated_at: 1, detail: EXPIRED, agent_id: '~ambient:claude' }),
      run({ updated_at: 2, status: 'completed' }),
    ])
    expect(broken.size).toBe(0)
  })

  it('ignores a one-off failure, but not the same failure every time', () => {
    expect(brokenAgents([run({ detail: 'model overloaded' })]).size).toBe(0)
    const gone = 'ACP request failed (code -32000): This authorization cannot access the requested session'
    const broken = brokenAgents([1, 2, 3].map(t => run({ updated_at: t, detail: gone })))
    expect(broken.get(healthKey('jess', 'claude'))?.problem).toBe('failing')
  })

  it('knows a used-up allowance', () => {
    expect(classifyFailure("Internal error: You've hit your session limit · resets 3:30am")).toBe('limited')
    expect(classifyFailure('429 rate limited, retry in 2s')).toBeNull()
  })
})
