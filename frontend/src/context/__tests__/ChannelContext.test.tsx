import { describe, expect, it, vi } from 'vitest'
import { act, render, screen } from '@testing-library/react'
import type { Channel } from '../../types'

const state = vi.hoisted(() => ({ activeCircleId: 'c1' as string | null }))
vi.mock('../AppContext', () => ({
  useApp: () => ({ activeCircleId: state.activeCircleId, status: null }),
}))

const pending = vi.hoisted(() => new Map<string, (list: Channel[]) => void>())
vi.mock('../../api', () => ({
  getChannels: vi.fn((id: string) => new Promise<Channel[]>(resolve => pending.set(id, resolve))),
  createChannel: vi.fn(),
  updateChannel: vi.fn(),
}))

import { ChannelProvider, useChannels } from '../ChannelContext'

const channel = (id: string | null, name: string): Channel => ({
  id, name, purpose: '', created_by: 'x', created_at: 0, archived: false,
})

function Names() {
  const { channels } = useChannels()
  return <div data-testid="names">{channels.map(c => c.name).join(',')}</div>
}

const names = () => screen.getByTestId('names').textContent

async function answer(circle: string, list: Channel[]) {
  await act(async () => { pending.get(circle)!(list); pending.delete(circle) })
}

describe('ChannelProvider', () => {
  it('never shows one circle\'s channels under another, and reuses a visited circle\'s list', async () => {
    const view = render(<ChannelProvider><Names /></ChannelProvider>)
    await answer('c1', [channel(null, 'general'), channel('a', 'design')])
    expect(names()).toBe('general,design')

    state.activeCircleId = 'c2'
    view.rerender(<ChannelProvider><Names /></ChannelProvider>)
    // c2 has not answered yet: nothing, rather than c1's channels.
    expect(names()).toBe('')
    await answer('c2', [channel(null, 'general'), channel('b', 'backend')])
    expect(names()).toBe('general,backend')

    state.activeCircleId = 'c1'
    view.rerender(<ChannelProvider><Names /></ChannelProvider>)
    // Back on c1: its list shows at once while the refresh is in flight.
    expect(names()).toBe('general,design')
    expect(pending.has('c1')).toBe(true)
  })
})
