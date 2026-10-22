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
  getAgentConfigFor: vi.fn(async () => ({ global_settings: { max_relay_turns: 6 }, circle: null })),
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

describe('the recipient token', () => {
  it('says which agent the next message reaches, in the field itself', async () => {
    getEngagement.mockResolvedValue({ agent: 'claude', peer_id: 'p1', window_secs: 180 })
    render(<ChatPanel />)
    const remove = await screen.findByRole('button', { name: 'Stop sending to @claude' })
    expect(remove.closest('.chat-composer__field')).toBeTruthy()
    expect(screen.getByTitle(/goes to it without a mention/)).toBeTruthy()
  })

  it('stays out of the way when there is nothing to follow up on', async () => {
    getEngagement.mockResolvedValue({ agent: null, window_secs: 180 })
    render(<ChatPanel />)
    await waitFor(() => expect(getEngagement).toHaveBeenCalled())
    expect(screen.queryByRole('button', { name: /Stop sending to/ })).toBeNull()
  })

  it('offers an exit, and taking it clears the token', async () => {
    getEngagement.mockResolvedValue({ agent: 'claude', peer_id: 'p1', window_secs: 180 })
    render(<ChatPanel />)

    const exit = await screen.findByRole('button', { name: 'Stop sending to @claude' })
    getEngagement.mockResolvedValue({ agent: null, window_secs: 180 })
    await userEvent.click(exit)

    await waitFor(() => expect(exitEngagement).toHaveBeenCalledWith('c1'))
    await waitFor(() => expect(screen.queryByRole('button', { name: /Stop sending to/ })).toBeNull())
  })

  it('Esc in the composer leaves the conversation', async () => {
    getEngagement.mockResolvedValue({ agent: 'claude', peer_id: 'p1', window_secs: 180 })
    render(<ChatPanel />)
    await screen.findByRole('button', { name: 'Stop sending to @claude' })

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
    expect(await screen.findByTitle(/your message will wait for it/)).toBeTruthy()
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

    expect(await screen.findByRole('link', { name: /Waiting for the message this replies to/ })).toHaveAttribute('href', '#chat-message-question')
    const reply = await screen.findByRole('button', { name: 'reply' })
    await userEvent.click(reply)

    expect(await screen.findByRole('button', { name: 'Stop sending to @suzy/mac/claude' })).toBeTruthy()
    expect(screen.getByTitle(/Goes to this agent only/)).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Stop sending to @codex' })).toBeNull()
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

describe('reply quotes', () => {
  const m = (id: string, reply_to?: string) => ({ id, reply_to, text: id, mentions: [], ts: 0, peer_id: 'p', agent_id: 'a' }) as never
  it('leaves out a quote position already makes', async () => {
    const { needsQuote } = await import('../ChatPanel')
    const thread = [m('q'), m('a1', 'q'), m('a2', 'q'), m('other'), m('late', 'a1')]
    expect(needsQuote(thread, 1)).toBe(false) // right under its question
    expect(needsQuote(thread, 2)).toBe(false) // among answers to the same one
    expect(needsQuote(thread, 4)).toBe(true) // something else came between
    expect(needsQuote([m('x', 'missing')], 0)).toBe(true) // its parent has not synced
    expect(needsQuote(thread, 0)).toBe(false) // not a reply
  })

  it('quotes one line, with a handle shortened the way a chip shows it', async () => {
    const { quoteLine } = await import('../ChatPanel')
    expect(quoteLine('\n  @suzy/mac/alpha is the retry path idempotent?\nmore')).toBe('@alpha is the retry path idempotent?')
  })
})

describe('who handed a turn over', () => {
  it('says "via" only when the agent before named this one', async () => {
    getEngagement.mockResolvedValue({ agent: null, window_secs: 0 })
    const { getChat, getMembers } = await import('../../api')
    ;(getMembers as ReturnType<typeof vi.fn>).mockResolvedValue([
      { peer_id: 'p1', owner: 'suzy', agent_id: 'suzy-mac', device_label: 'mac', agents: ['alpha', 'beta', 'gamma'], role: 'admin', added_at: '', signature: '' },
    ])
    const now = Math.floor(Date.now() / 1000)
    const agent = (id: string, who: string, reply_to: string, path: string[], mentions: string[] = []) =>
      ({ id, agent_id: who, text: `${who} says`, reply_to, mentions, ts: now, peer_id: 'p1', author: 'agent', relay: { root: 'r', root_peer: 'p1', spent: path.length, path } })
    ;(getChat as ReturnType<typeof vi.fn>).mockResolvedValue([
      agent('a', 'alpha', 'q', ['alpha'], ['gamma']),
      agent('b', 'beta', 'a', ['alpha', 'beta']),
      agent('g', 'gamma', 'a', ['alpha', 'gamma']),
    ])
    render(<ChatPanel />)
    expect(await screen.findByText('after alpha')).toBeTruthy() // the warden chose beta
    expect(await screen.findByText('via @alpha')).toBeTruthy() // alpha named gamma
  })
})

describe('a conversation among agents', () => {
  it('shows how far along it is, with its brake in the same pill', async () => {
    getEngagement.mockResolvedValue({ agent: null, window_secs: 0 })
    const { getChat, getMembers, stopRelay } = await import('../../api')
    // This device runs beta, so its own limit is the one that applies.
    ;(getMembers as ReturnType<typeof vi.fn>).mockResolvedValue([
      { peer_id: 'p1', owner: 'alice', agent_id: 'alice', device_label: 'mac', agents: ['beta'], role: 'admin', added_at: '', signature: '' },
    ])
    const now = Math.floor(Date.now() / 1000)
    ;(getChat as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 'a', agent_id: 'alpha', text: 'done?', reply_to: 'q', mentions: [], ts: now, peer_id: 'p1', author: 'agent', relay: { root: 'q', root_peer: 'p1', spent: 1, path: ['alpha'] } },
    ])
    getChatActivity.mockResolvedValue([
      { activity_id: 'w', actor_id: 'beta', peer_id: 'p1', kind: 'working', message_id: 'a', updated_at: now + 60, expires_at: now + 60 },
    ] as never)
    render(<ChatPanel />)
    expect(await screen.findByText(/agents talking · turn 2 of 6/)).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: 'stop' }))
    await waitFor(() => expect(stopRelay).toHaveBeenCalledWith('c1', 'q'))
  })

  it('leaves out the limit when another device runs the turn', async () => {
    getEngagement.mockResolvedValue({ agent: null, window_secs: 0 })
    const { getChat, getMembers } = await import('../../api')
    ;(getMembers as ReturnType<typeof vi.fn>).mockResolvedValue([
      { peer_id: 'p1', owner: 'alice', agent_id: 'alice', device_label: 'mac', agents: [], role: 'admin', added_at: '', signature: '' },
    ])
    const now = Math.floor(Date.now() / 1000)
    ;(getChat as ReturnType<typeof vi.fn>).mockResolvedValue([
      { id: 'a', agent_id: 'alpha', text: 'done?', reply_to: 'q', mentions: [], ts: now, peer_id: 'p1', author: 'agent', relay: { root: 'q', root_peer: 'p1', spent: 7, path: ['alpha'] } },
    ])
    getChatActivity.mockResolvedValue([
      { activity_id: 'w', actor_id: 'beta', peer_id: 'p9', kind: 'working', message_id: 'a', updated_at: now + 60, expires_at: now + 60 },
    ] as never)
    render(<ChatPanel />)
    expect(await screen.findByText(/agents talking · turn 8$/)).toBeTruthy()
    expect(screen.queryByText(/of 6/)).toBeNull()
  })
})
