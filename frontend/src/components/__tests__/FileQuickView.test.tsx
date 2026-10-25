import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

vi.mock('../../context/AppContext', () => ({ useApp: () => ({ activeCircleId: 'circle' }) }))
vi.mock('../../api', () => ({ wsYjsUrl: (_circle: string, path: string) => path, getMembers: vi.fn() }))
vi.mock('../../lib/YjsProvider', () => ({
  YjsProvider: class {
    constructor(path: string, doc: { getText: (path: string) => { insert: (index: number, text: string) => void } }, synced: () => void) {
      doc.getText(path).insert(0, path.endsWith('.html') ? '<h1>HTML content</h1>' : '# Shared document\n\nA readable preview.')
      synced()
    }
    destroy() {}
  },
}))
const { FileQuickView } = await import('../EditorPanel')

describe('sidebar document navigation', () => {
  it('returns to Files and carries the selected mode to the center', async () => {
    const open = vi.fn(), back = vi.fn()
    render(<FileQuickView filePath="notes.md" full onOpen={open} onClose={back} />)
    expect(await screen.findByRole('heading', { name: 'Shared document' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: /Close/ })).toBeNull()
    await userEvent.click(screen.getByRole('button', { name: 'Open in center' }))
    expect(open).toHaveBeenLastCalledWith('preview')
    await userEvent.click(screen.getByRole('tab', { name: 'SOURCE' }))
    await userEvent.click(screen.getByRole('button', { name: 'Open in center' }))
    expect(open).toHaveBeenLastCalledWith('source')
    await userEvent.click(screen.getByRole('button', { name: 'Back to files' }))
    expect(back).toHaveBeenCalledOnce()
  })
  it('explains HTML restrictions and retains the sandbox', () => {
    render(<FileQuickView filePath="page.html" full onOpen={vi.fn()} onClose={vi.fn()} />)
    expect(screen.getByText(/scripts and network disabled/)).toBeTruthy()
    expect(screen.getByTitle('Preview of page.html')).toHaveAttribute('sandbox', '')
    expect(screen.getByTitle('Preview of page.html').getAttribute('srcdoc')).toContain("default-src 'none'")
  })
})
