import { describe, it, expect } from 'vitest'
import { shortenAgentId, peerLabel, resolveDisplayName } from '../displayName'
import type { Member } from '../../types'

const member = (over: Partial<Member>): Member => ({
  peer_id: 'p', owner: 'suzy', agent_id: 'suzy-AbCdEfGh',
  device_label: 'mac', agents: [], role: 'member', added_at: '', signature: '',
  ...over,
} as Member)

describe('shortenAgentId', () => {
  it('trims the 8-char suffix down to 4', () => {
    expect(shortenAgentId('suzy-AbCdEfGh')).toBe('suzy-AbCd')
  })

  it('leaves an id without that suffix shape alone', () => {
    expect(shortenAgentId('suzy')).toBe('suzy')
    expect(shortenAgentId('suzy-Ab')).toBe('suzy-Ab')
  })
})

describe('peerLabel', () => {
  it('prefers the owner name', () => {
    expect(peerLabel('suzy', 'suzy-AbCdEfGh')).toBe('suzy')
  })

  it('trims surrounding whitespace on the owner', () => {
    expect(peerLabel('  suzy  ', 'suzy-AbCdEfGh')).toBe('suzy')
  })

  it('falls back to the shortened agent id when the owner is blank', () => {
    expect(peerLabel('   ', 'suzy-AbCdEfGh')).toBe('suzy-AbCd')
  })

  it('rejects an implausibly long owner rather than letting it into the UI', () => {
    expect(peerLabel('x'.repeat(41), 'suzy-AbCdEfGh')).toBe('suzy-AbCd')
  })
})

describe('resolveDisplayName', () => {
  it('uses the matching member', () => {
    expect(resolveDisplayName('suzy-AbCdEfGh', [member({ owner: 'jessair' })]))
      .toBe('jessair')
  })

  it('shortens the id when no member matches', () => {
    expect(resolveDisplayName('ghost-AbCdEfGh', [])).toBe('ghost-AbCd')
  })
})
