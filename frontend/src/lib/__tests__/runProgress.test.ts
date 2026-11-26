import { describe, expect, it } from 'vitest'
import { describeProgress } from '../runProgress'
import type { RunProgress } from '../../types'

const base: RunProgress = { started_at: 0, last_activity_at: null, tool: null, open_tools: 0, plan_done: 0, plan_total: 0, plan_step: null }

describe('describeProgress', () => {
  it('says nothing before the agent has reported anything', () => {
    expect(describeProgress(undefined)).toBeUndefined()
    expect(describeProgress(base)).toBeUndefined()
  })

  it('names the program for execute and the path for file tools', () => {
    expect(describeProgress({ ...base, open_tools: 1, tool: { kind: 'execute', program: 'cargo', path: null, since: 1 } }))
      .toBe('running cargo')
    expect(describeProgress({ ...base, open_tools: 1, tool: { kind: 'read', program: null, path: 'notes/plan.md', since: 1 } }))
      .toBe('reading notes/plan.md')
    expect(describeProgress({ ...base, open_tools: 3, tool: { kind: 'search', program: null, path: null, since: 1 } }))
      .toBe('searching +2')
    expect(describeProgress({ ...base, open_tools: 1, tool: { kind: 'other', program: null, path: null, since: 1 } }))
      .toBe('using a tool')
  })

  it('counts the plan from the step in progress', () => {
    expect(describeProgress({ ...base, plan_done: 1, plan_total: 3, plan_step: 'Write the tests' }))
      .toBe('step 2/3: Write the tests')
    expect(describeProgress({ ...base, plan_done: 3, plan_total: 3 })).toBe('step 3/3')
    expect(describeProgress({ ...base, open_tools: 1, plan_done: 0, plan_total: 2, plan_step: 'Build',
      tool: { kind: 'execute', program: 'npm', path: null, since: 1 } }))
      .toBe('running npm · step 1/2: Build')
  })
})
