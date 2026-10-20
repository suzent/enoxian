/**
 * The composer must say what the next message will do (§1.2).
 *
 * Implicit routing that is invisible is a bug: a user who cannot see that their
 * next line will wake an agent has lost the ability to decide not to.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'

const getEngagement = vi.fn()
const exitEngagement = vi.fn()
const getChatActivity = vi.fn(async () => [])

vi.mock('../../api', () => ({
  postChat: vi.fn(async () => ({ id: 'm1' })),
  getExecutions: vi.fn(async () => ({ runs: [] })),
  updateExecution: vi.fn(),
  getChat: vi.fn(async () => []),
  getMembers: vi.fn(async () => []),
  getWho: vi.fn(async () => []),
  getChatActivity,
  setChatTyping: vi.fn(async () => ({ ok: true })),
  uploadAttachment: vi.fn(),
  stopRelay: vi.fn(async () => ({ ok: true, root: 'r' })),
  getEngagement,
  exitEngagement,
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

const { default: ChatPanel } = await import('../ChatPanel')

beforeEach(() => {
  getEngagement.mockReset()
  exitEngagement.mockReset()
  exitEngagement.mockResolvedValue({ ok: true })
})

describe('the follow-up strip', () => {
  it('says which agent the next message reaches', async () => {
    getEngagement.mockResolvedValue({ agent: 'claude', peer_id: 'p1', window_secs: 180 })
    render(<ChatPanel />)
    expect(await screen.findByText(/replying to/)).toBeTruthy()
    expect(await screen.findByText('@claude')).toBeTruthy()
    expect(await screen.findByText(/no mention needed/)).toBeTruthy()
  })

  it('stays out of the way when there is nothing to follow up on', async () => {
    getEngagement.mockResolvedValue({ agent: null, window_secs: 180 })
    render(<ChatPanel />)
    await waitFor(() => expect(getEngagement).toHaveBeenCalled())
    expect(screen.queryByText(/replying to/)).toBeNull()
  })

  it('offers an exit, and taking it clears the strip', async () => {
    getEngagement.mockResolvedValue({ agent: 'claude', peer_id: 'p1', window_secs: 180 })
    render(<ChatPanel />)

    const exit = await screen.findByRole('button', { name: /esc to exit/ })
    getEngagement.mockResolvedValue({ agent: null, window_secs: 180 })
    await userEvent.click(exit)

    await waitFor(() => expect(exitEngagement).toHaveBeenCalledWith('c1'))
    await waitFor(() => expect(screen.queryByText(/replying to/)).toBeNull())
  })

  it('Esc in the composer leaves the conversation', async () => {
    getEngagement.mockResolvedValue({ agent: 'claude', peer_id: 'p1', window_secs: 180 })
    render(<ChatPanel />)
    await screen.findByText('@claude')

    const box = await screen.findByRole('textbox')
    await userEvent.click(box)
    getEngagement.mockResolvedValue({ agent: null, window_secs: 180 })
    await userEvent.keyboard('{Escape}')

    await waitFor(() => expect(exitEngagement).toHaveBeenCalled())
  })

  it('warns that a message to a busy agent will be queued rather than fail', async () => {
    getEngagement.mockResolvedValue({ agent: 'claude', peer_id: 'p1', window_secs: 180 })
    const soon = Math.floor(Date.now() / 1000) + 60
    getChatActivity.mockResolvedValue([
      {
        activity_id: 'a1',
        actor_id: 'claude',
        peer_id: 'p2',
        kind: 'working',
        message_id: 'm1',
        updated_at: soon,
        expires_at: soon,
      },
    ] as never)
    render(<ChatPanel />)
    expect(await screen.findByText(/will be queued/)).toBeTruthy()
  })
})

describe('explicit reply-to', () => {
  it('outranks the window: pointing beats guessing', async () => {
    // The window says codex; the user points at claude's message.
    getEngagement.mockResolvedValue({ agent: 'codex', peer_id: 'p1', window_secs: 180 })
    const { getChat, getMembers } = await import('../../api')
    // The panel resolves an agent message through the roster, so the posting
    // device has to be in it for the message to read as agent-authored.
    ;(getMembers as ReturnType<typeof vi.fn>).mockResolvedValue([
      {
        peer_id: 'p1',
        owner: 'suzy',
        agent_id: 'suzy-mac',
        device_label: 'mac',
        agents: ['claude'],
        role: 'admin',
        added_at: '',
        signature: '',
      },
    ])
    ;(getChat as ReturnType<typeof vi.fn>).mockResolvedValue([
      {
        id: 'a1',
        agent_id: 'claude',
        text: 'from claude',
        reply_to: 'question',
        mentions: [],
        ts: Math.floor(Date.now() / 1000),
        peer_id: 'p1',
        author: 'agent',
        relay: { root: 'r', root_peer: 'p1', spent: 1, path: ['claude'] },
      },
    ])
    render(<ChatPanel />)

    expect(await screen.findByRole('link', { name: /Replying to\s*Waiting for the referenced message to sync/ })).toHaveAttribute('href', '#chat-message-question')
    const reply = await screen.findByRole('button', { name: 'reply' })
    await userEvent.click(reply)

    expect(await screen.findByText('@suzy/mac/claude')).toBeTruthy()
    expect(await screen.findByText(/this message only/)).toBeTruthy()
    expect(screen.queryByText('@codex')).toBeNull()
  })
})

describe('the activity strip', () => {
  it('says each kind of activity once when several agents scry the room', async () => {
    getEngagement.mockResolvedValue({ agent: null, window_secs: 180 })
    const soon = Math.floor(Date.now() / 1000) + 60
    const activity = (actor: string, kind: string, detail?: string) => ({
      activity_id: `${actor}-${kind}`, actor_id: actor, peer_id: 'p2', kind, detail,
      message_id: 'm1', updated_at: soon, expires_at: soon,
    })
    const { getMembers } = await import('../../api')
    ;(getMembers as ReturnType<typeof vi.fn>).mockResolvedValue([{
      peer_id: 'p2', owner: 'bob', agent_id: 'bob-box', device_label: 'box',
      agents: ['claude', 'codex', 'gemini', 'aider', 'cursor'], role: 'member', added_at: '', signature: '',
    }])
    getChatActivity.mockResolvedValue([
      activity('claude', 'working'), activity('codex', 'working'), activity('gemini', 'working'),
      activity('aider', 'working'), activity('cursor', 'skipped', 'not addressed'),
    ] as never)
    render(<ChatPanel />)
    expect(await screen.findByText('4 agents')).toBeTruthy()
    expect(screen.getByText('are working…')).toBeTruthy()
    expect(screen.getAllByText(/working…/)).toHaveLength(1)
    // Four is past the names the strip lists, so it counts them instead.
    const strip = screen.getByText('4 agents').closest('.chat-activity') as HTMLElement
    expect(within(strip).queryByText('claude')).toBeNull()
    expect(within(strip).getByText('cursor')).toBeTruthy()
    // A lone pass keeps its reason on the strip; it is the whole answer.
    expect(screen.getByText(/not addressed/)).toBeTruthy()
  })

  it('gets terser instead of wrapping when the line is too narrow', async () => {
    getEngagement.mockResolvedValue({ agent: null, window_secs: 180 })
    const soon = Math.floor(Date.now() / 1000) + 60
    const activity = (actor: string, kind: string) => ({
      activity_id: `${actor}-${kind}`, actor_id: actor, peer_id: 'p2', kind,
      message_id: 'm1', updated_at: soon, expires_at: soon,
    })
    getChatActivity.mockResolvedValue([
      activity('claude', 'working'), activity('codex', 'working'),
      activity('cursor', 'skipped'), activity('aider', 'skipped'), activity('bob', 'typing'),
    ] as never)
    // jsdom lays nothing out; pretend the strip never fits.
    const scroll = vi.spyOn(HTMLElement.prototype, 'scrollWidth', 'get').mockReturnValue(1000)
    const client = vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(300)
    try {
      render(<ChatPanel />)
      const strip = (await screen.findByText('working')).closest('.chat-activity') as HTMLElement
      expect(within(strip).getAllByText('2')).toHaveLength(2)
      expect(within(strip).getByText('skipped')).toBeTruthy()
      // Typing is the last thing to go, and it went: still no second line.
      expect(within(strip).queryByText(/typing/)).toBeNull()
    } finally {
      scroll.mockRestore()
      client.mockRestore()
    }
  })
})
