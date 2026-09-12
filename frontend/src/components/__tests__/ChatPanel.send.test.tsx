/**
 * A failed send must not cost the user their message.
 *
 * The composer is cleared optimistically — it has to be, or sending feels
 * laggy — so the only thing standing between a moment of control-doc
 * contention and a destroyed message is the restore path below.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'

const postChat = vi.fn()

vi.mock('../../api', () => ({
  postChat,
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
  chatStream: () => ({
    onopen: null,
    onerror: null,
    addEventListener: vi.fn(),
    close: vi.fn(),
  }),
}))

vi.mock('../../context/AppContext', () => ({
  useApp: () => ({
    activeCircleId: 'c1',
    circles: [{ circle_id: 'c1', circle_name: 'Test', disabled: false }],
    status: { agent_id: 'alice' },
  }),
}))

const { default: ChatPanel } = await import('../ChatPanel')

async function typeMessage(text: string) {
  const box = await screen.findByRole('textbox')
  await userEvent.click(box)
  await userEvent.keyboard(text)
  return box
}

beforeEach(() => {
  postChat.mockReset()
})

describe('a send that fails', () => {
  it('puts the message back in the composer instead of destroying it', async () => {
    postChat.mockRejectedValue(new Error('Circle state is busy syncing. Try again shortly.'))
    render(<ChatPanel />)

    const box = await typeMessage('a long message worth keeping')
    await userEvent.keyboard('{Enter}')

    await waitFor(() => expect(postChat).toHaveBeenCalled())
    // The text is back, and the user is told why — in the daemon's words.
    await waitFor(() => expect(box).toHaveTextContent('a long message worth keeping'))
    expect(await screen.findByText(/busy syncing/)).toBeTruthy()
  })

  it('clears the composer and says nothing when the send succeeds', async () => {
    postChat.mockResolvedValue({ id: 'm1' })
    render(<ChatPanel />)

    const box = await typeMessage('fire and forget')
    await userEvent.keyboard('{Enter}')

    await waitFor(() => expect(postChat).toHaveBeenCalled())
    await waitFor(() => expect(box).toHaveTextContent(''))
    expect(screen.queryByText(/Not sent/)).toBeNull()
  })
})
