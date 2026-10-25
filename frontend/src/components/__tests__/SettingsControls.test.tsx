import { fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, it, expect, vi } from 'vitest'
import { useState } from 'react'
import NumberField from '../ui/NumberField'
import Dialog, { ConfirmDialog } from '../ui/Dialog'
import Select from '../ui/Select'

describe('settings input lifecycle', () => {
  it('allows a multi-digit draft and commits once on Enter', async () => {
    const commit = vi.fn()
    render(<NumberField value={2} min={1} max={50} aria-label="Limit" onCommit={commit} />)
    const input = screen.getByRole('spinbutton')
    await userEvent.clear(input)
    await userEvent.type(input, '32')
    expect(commit).not.toHaveBeenCalled()
    expect(input).toHaveValue(32)
    await userEvent.keyboard('{Enter}')
    expect(commit).toHaveBeenCalledExactlyOnceWith(32)
  })
  it('restores empty input and clamps an out-of-range number', () => {
    const commit = vi.fn()
    render(<NumberField value={20} min={1} max={50} aria-label="Limit" onCommit={commit} />)
    const input = screen.getByRole('spinbutton')
    fireEvent.change(input, { target: { value: '' } })
    fireEvent.blur(input)
    expect(input).toHaveValue(20)
    expect(commit).not.toHaveBeenCalled()
    fireEvent.change(input, { target: { value: '500' } })
    fireEvent.blur(input)
    expect(commit).toHaveBeenCalledWith(50)
  })
  it('Escape dismisses a select before the enclosing dialog', async () => {
    const close = vi.fn()
    render(<Dialog title="Settings" onClose={close}>
      <Select aria-label="Mode" value="a" options={[{ value: 'a', label: 'Automatic' }]} onChange={vi.fn()} />
    </Dialog>)
    await userEvent.click(screen.getByRole('combobox'))
    await userEvent.keyboard('{Escape}')
    expect(screen.queryByRole('listbox')).toBeNull()
    expect(close).not.toHaveBeenCalled()
    await userEvent.keyboard('{Escape}')
    expect(close).toHaveBeenCalledOnce()
  })
  it('nested confirmation closes alone and returns focus to its trigger', async () => {
    const close = vi.fn()
    function Example() {
      const [confirm, setConfirm] = useState(false)
      return <Dialog title="Settings" onClose={close}>
        <button onClick={() => setConfirm(true)}>Enable listener</button>
        {confirm && <ConfirmDialog title="Confirm" onConfirm={vi.fn()} onClose={() => setConfirm(false)}>Explanation</ConfirmDialog>}
      </Dialog>
    }
    render(<Example />)
    await userEvent.click(screen.getByText('Enable listener'))
    await userEvent.keyboard('{Escape}')
    expect(screen.queryByRole('dialog', { name: 'Confirm' })).toBeNull()
    expect(screen.getByText('Enable listener')).toHaveFocus()
    expect(close).not.toHaveBeenCalled()
  })
  it('restores the app when parent and confirmation unmount together', async () => {
    const root = document.createElement('div')
    root.id = 'root'
    root.inert = false
    document.body.append(root)
    function Example() {
      const [open, setOpen] = useState(true)
      const [confirm, setConfirm] = useState(false)
      return open && <>
        <Dialog title="Settings" onClose={() => setConfirm(true)}><button>Field</button></Dialog>
        {confirm && <ConfirmDialog title="Discard?" onClose={() => setConfirm(false)} onConfirm={() => setOpen(false)}>Unsaved draft</ConfirmDialog>}
      </>
    }
    const { unmount } = render(<Example />)
    expect(root.inert).toBe(true)
    await userEvent.keyboard('{Escape}')
    await userEvent.click(screen.getByRole('button', { name: 'Continue' }))
    expect(root.inert).toBe(false)
    unmount()
    root.remove()
  })
})
