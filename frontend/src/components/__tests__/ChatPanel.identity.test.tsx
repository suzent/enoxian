import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'

let receive: (event: {data: string}) => void
const old = { id: 'old', agent_id: 'bob', text: 'History', ts: 1, mentions: [] }
vi.mock('../../api', () => ({
  getChat: vi.fn(async () => [old]),
  getMembers: vi.fn(async () => []), getWho: vi.fn(async () => []),
  getChatActivity: vi.fn(async () => []),
  getEngagement: vi.fn(async () => ({agent: null, window_secs: 180})),
  getExecutions: vi.fn(async () => ({runs: []})),
  chatStream: () => ({onopen: null, close: vi.fn(), addEventListener: (_: string, cb: typeof receive) => { receive = cb }}),
  postChat: vi.fn(), setChatTyping: vi.fn(), uploadAttachment: vi.fn(), stopRelay: vi.fn(), exitEngagement: vi.fn(),
  blobUrl: vi.fn(), MAX_ATTACHMENT_BYTES: 100,
}))
vi.mock('../../context/AppContext', () => ({useApp: () => ({
  activeCircleId: 'circle', circles: [{circle_id: 'circle', circle_name: 'Design'}], status: {agent_id: 'alice'},
})}))
vi.mock('../CircleGlyph', () => ({default: () => <span />}))
const { default: ChatPanel } = await import('../ChatPanel')
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals() })

it('only signals unseen incoming live messages, then clears when the transcript is read', async () => {
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { cb(0); return 1 })
  vi.spyOn(document, 'hasFocus').mockReturnValue(false)
  const view = render(<ChatPanel variant="main" />)
  await screen.findByText('History')
  const notify = (id: string, agent_id = 'bob') => act(() => receive({data: JSON.stringify({type:'message_posted',message:{...old,id,agent_id,text:id}})}))
  expect(screen.queryByText('New messages')).toBeNull()
  notify('old')
  notify('self', 'alice')
  expect(screen.queryByText('New messages')).toBeNull()
  notify('incoming')
  await waitFor(() => expect(screen.getByText('New messages')).toBeInTheDocument())
  fireEvent.scroll(view.container.querySelector('.chat-message-list')!)
  expect(screen.queryByText('New messages')).toBeNull()
  notify('incoming')
  expect(screen.queryByText('New messages')).toBeNull()
})

it('routes each workspace shortcut to its own contextual destination', async () => {
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { cb(0); return 1 })
  const navigate = vi.fn()
  render(<ChatPanel variant="main" onOpenCircleDetails={navigate} />)
  await screen.findByText('History')
  fireEvent.click(screen.getByRole('button', { name: 'View activity for Design' }))
  expect(navigate).toHaveBeenLastCalledWith('activity')
  fireEvent.click(screen.getByRole('button', { name: 'View 0 members, 0 online' }))
  expect(navigate).toHaveBeenLastCalledWith('members')
  fireEvent.click(screen.getByRole('button', { name: /^Tasks$/ }))
  expect(navigate).toHaveBeenLastCalledWith('tasks')
  fireEvent.click(screen.getByRole('button', { name: /^Workspace$/ }))
  expect(navigate).toHaveBeenLastCalledWith('workspace')
})
