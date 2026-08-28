import { describe, it, expect } from 'vitest'
import { buildMentionItems } from '../MentionPopup'
import type { Member, Presence } from '../../types'

const member = (over: Partial<Member>): Member => ({
  peer_id: 'p1', owner: 'suzy', agent_id: 'suzy-AbCdEfGh', device_label: 'mac',
  agents: [], role: 'member', added_at: '', signature: '', ...over,
} as Member)

const presence = (over: Partial<Presence>): Presence => ({
  agent_id: 'suzy-AbCdEfGh', status: 'online', last_seen: '', peer_id: 'p1', ...over,
} as Presence)

describe('buildMentionItems', () => {
  it('nests owner → device → agent, and only agents are runnable', () => {
    const items = buildMentionItems([member({ agents: ['claude'] })], [], '')
    expect(items.map(i => [i.level, i.insert])).toEqual([
      [0, 'suzy'],
      [1, 'suzy/mac'],
      [2, 'suzy/mac/claude'],
    ])
    expect(items.filter(i => i.runnable).map(i => i.insert)).toEqual(['suzy/mac/claude'])
  })

  it('groups several devices under one owner', () => {
    const items = buildMentionItems([
      member({ peer_id: 'p1', device_label: 'mac' }),
      member({ peer_id: 'p2', device_label: 'jessair', agent_id: 'suzy-ZzZzZzZz' }),
    ], [], '')
    expect(items.filter(i => i.level === 0)).toHaveLength(1)
    expect(items.filter(i => i.level === 1).map(i => i.insert))
      .toEqual(['suzy/mac', 'suzy/jessair'])
  })

  it('takes online state from the peer id, not just the agent id', () => {
    const items = buildMentionItems(
      [member({ peer_id: 'p1', agents: ['claude'] })],
      [presence({ peer_id: 'p1', agent_id: 'someone-else', status: 'online' })],
      '',
    )
    expect(items.find(i => i.level === 2)?.online).toBe(true)
  })

  it('treats a peer with no presence entry as offline', () => {
    const items = buildMentionItems([member({ agents: ['claude'] })], [], '')
    expect(items.find(i => i.level === 2)?.online).toBe(false)
  })

  it('filters on the label so typing an agent name surfaces it', () => {
    const items = buildMentionItems([member({ agents: ['claude', 'codex'] })], [], 'claude')
    expect(items.map(i => i.insert)).toEqual(['suzy/mac/claude'])
  })

  it('filters on the token so a path narrows to one device subtree', () => {
    const items = buildMentionItems([
      member({ peer_id: 'p1', device_label: 'mac', agents: ['claude'] }),
      member({ peer_id: 'p2', device_label: 'jessair', agent_id: 'suzy-Zz', agents: ['claude'] }),
    ], [], 'suzy/mac')
    expect(items.map(i => i.insert)).toEqual(['suzy/mac', 'suzy/mac/claude'])
  })

  it('is case-insensitive', () => {
    expect(buildMentionItems([member({ agents: ['Claude'] })], [], 'CLAUDE')
      .map(i => i.insert)).toEqual(['suzy/mac/Claude'])
  })
})
