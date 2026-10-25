import { render, screen, fireEvent } from '@testing-library/react'
import { describe, it, expect, vi } from 'vitest'
import { useState } from 'react'
import ActionMenu from '../ui/ActionMenu'

describe('shared file menu', () => {
  it('supports arrow navigation, selection and Escape without closing its parent', () => {
    const selected = vi.fn(), parentKey = vi.fn()
    function Menu() {
      const [open, setOpen] = useState(false)
      return <div onKeyDown={parentKey}><button onClick={() => setOpen(true)}>Actions</button>
        {open && <ActionMenu label="File actions" anchor={{ x: 30, y: 30, trigger: screen.getByRole('button', { name: 'Actions' }) }}
          actions={[{ label: 'Copy path', onSelect: selected }, { label: 'Rename', onSelect: selected }]}
          onClose={() => setOpen(false)} />}
      </div>
    }
    render(<Menu />)
    fireEvent.click(screen.getByRole('button'))
    expect(screen.getByRole('menuitem', { name: 'Copy path' })).toHaveFocus()
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' })
    expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus()
    parentKey.mockClear()
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' })
    expect(screen.queryByRole('menu')).toBeNull()
    expect(screen.getByRole('button')).toHaveFocus()
    expect(parentKey).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button'))
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy path' }))
    expect(selected).toHaveBeenCalledOnce()
    expect(screen.queryByRole('menu')).toBeNull()
  })
})
