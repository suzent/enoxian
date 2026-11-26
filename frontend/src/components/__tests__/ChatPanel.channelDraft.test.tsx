/**
 * The composer's draft belongs to the channel it was written in. The panel
 * stays mounted across a channel switch, so without this a half-written
 * message would follow you into the next channel — and send there.
 */
import { describe, it, expect, vi } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'

const postChat = vi.fn(async () => ({ id: 'm1' }))

vi.mock('../../api', () => ({
  getAgentConfigFor: vi.fn(async () => ({ global_settings: { max_relay_turns: 20 }, circle: null })),
  postChat,
  getExecutions: vi.fn(async () => ({ runs: [] })),
  updateExecution: vi.fn(),
  getChat: vi.fn(async () => []),
  getMembers: vi.fn(async () => []),
  getWho: vi.fn(async () => []),
  getChatActivity: vi.fn(async () => []),
  setChatTyping: vi.fn(async () => ({ ok: true })),
  uploadAttachment: vi.fn(),
  stopRelay: vi.fn(async () => ({ ok: true, root: 'r' })),
  getEngagement: vi.fn(async () => ({ agent: null, window_secs: 180 })),
  exitEngagement: vi.fn(async () => ({ ok: true })),
  blobUrl: (_c: string, h: string) => `/blob/${h}`,
  MAX_ATTACHMENT_BYTES: 10 * 1024 * 1024,
  chatStream: () => ({ onopen: null, onerror: null, addEventListener: vi.fn(), close: vi.fn() }),
}))

vi.mock('../../context/AppContext', () => ({
  useApp: () => ({
    activeCircleId: 'c1',
    circles: [{ circle_id: 'c1', circle_name: 'Test', disabled: false }],
    status: { agent_id: 'alice' },
  }),
}))

// Stable values: the panel's effects depend on these, as they do on the real
// context's memoized ones.
const open = vi.hoisted(() => {
  const noop = () => {}
  const asyncNoop = async () => {}
  return {
    channel: null as string | null,
    channels: [
      { id: null, name: 'general', purpose: '', created_by: '', created_at: 0, archived: false },
      { id: 'c-design', name: 'design', purpose: '', created_by: '', created_at: 0, archived: false },
    ],
    unread: {},
    noop,
    asyncNoop,
  }
})
vi.mock('../../context/ChannelContext', () => ({
  channelKey: (id: string | null | undefined) => id ?? '',
  useChannels: () => ({
    channels: open.channels,
    channel: open.channel,
    unread: open.unread,
    selectChannel: open.noop,
    createChannel: open.asyncNoop,
    archiveChannel: open.asyncNoop,
    refreshChannels: open.noop,
    noteUnread: open.noop,
  }),
}))

const { default: ChatPanel } = await import('../ChatPanel')

describe('composer drafts', () => {
  it('stay in the channel they were written in', async () => {
    const { rerender } = render(<ChatPanel />)
    const box = await screen.findByRole('textbox')
    await userEvent.click(box)
    await userEvent.keyboard('half a thought for general')

    open.channel = 'c-design'
    rerender(<ChatPanel />)
    await waitFor(() => expect(screen.getByRole('textbox')).toHaveTextContent(''))

    open.channel = null
    rerender(<ChatPanel />)
    await waitFor(() => expect(screen.getByRole('textbox')).toHaveTextContent('half a thought for general'))
    expect(postChat).not.toHaveBeenCalled()
  })
})
