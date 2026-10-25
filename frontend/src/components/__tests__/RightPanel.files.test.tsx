import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import RightPanel from '../RightPanel'

vi.mock('../../api', async importOriginal => ({
  ...await importOriginal<typeof import('../../api')>(),
  getWho: vi.fn(async () => []), getTasks: vi.fn(async () => []),
  getFiles: vi.fn(async () => ['notes/brief.md']), getMembers: vi.fn(async () => []),
  getPending: vi.fn(async () => []), getProposals: vi.fn(async () => []),
  eventStream: () => ({ addEventListener: vi.fn(), close: vi.fn() }),
}))
vi.mock('../../context/AppContext', () => ({ useApp: () => ({
  activeCircleId: 'c1', circles: [{ circle_id: 'c1', circle_name: 'Test' }],
  reloadCircles: vi.fn(), status: { agent_id: 'self' },
}) }))

describe('file interaction entry points', () => {
  it('opens the same actions from right-click and overflow and copies the full relative path', async () => {
    const writeText = vi.fn(async () => {})
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    render(<RightPanel onFileSelect={vi.fn()} selectedFile={null} activeTab="workspace" onClose={vi.fn()} />)
    fireEvent.click(await screen.findByRole('button', { name: 'notes' }))
    fireEvent.contextMenu(screen.getByRole('button', { name: 'brief.md' }), { clientX: 80, clientY: 90 })
    expect(screen.getAllByRole('menuitem').map(item => item.textContent)).toEqual(['Open in center', 'Copy file path', 'Rename', 'Delete'])
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy file path' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('notes/brief.md'))
    expect(await screen.findByText('Copied Circle-relative path.')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'More actions for notes/brief.md' }))
    expect(screen.getAllByRole('menuitem')).toHaveLength(4)
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' })
    fireEvent.click(screen.getByRole('button', { name: 'New file' }))
    expect(screen.getByRole('menuitem', { name: 'Upload text files…' })).toBeTruthy()
    fireEvent.click(screen.getByRole('menuitem', { name: 'Create blank file' }))
    expect(screen.getByRole('textbox', { name: 'New file path' })).toBeTruthy()
  })
})
