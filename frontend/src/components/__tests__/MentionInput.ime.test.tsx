import { describe, it, expect, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import { createRef } from 'react'
import MentionInput, { type MentionInputHandle } from '../MentionInput'

/**
 * Regression tests for the IME composition guard.
 *
 * Typing Chinese, Japanese or Korean routes keystrokes through a candidate
 * window. Enter there means "accept this candidate" and the arrows move
 * between candidates — neither belongs to the app. Before the guard, the chat
 * input read Enter as "send", so a message was posted on the first character
 * anyone typed.
 *
 * These drive the composition signals directly because jsdom has no IME: a
 * real one sets `isComposing` on the key events it generates, and some
 * browsers report only the legacy `keyCode === 229`. Both are covered.
 */
function setup() {
  const onKeyDown = vi.fn()
  const onChange = vi.fn()
  const ref = createRef<MentionInputHandle>()
  render(
    <MentionInput ref={ref} placeholder="msg" onChange={onChange} onKeyDown={onKeyDown} />,
  )
  return { onKeyDown, onChange, editor: screen.getByRole('textbox') }
}

function keyDown(el: Element, init: KeyboardEventInit) {
  el.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init }))
}

describe('MentionInput IME composition guard', () => {
  it('does not forward Enter while a composition is in flight', () => {
    const { onKeyDown, editor } = setup()
    keyDown(editor, { key: 'Enter', isComposing: true })
    expect(onKeyDown).not.toHaveBeenCalled()
  })

  it('does not forward Enter reported only via the legacy keyCode 229', () => {
    const { onKeyDown, editor } = setup()
    keyDown(editor, { key: 'Enter', keyCode: 229 })
    expect(onKeyDown).not.toHaveBeenCalled()
  })

  it('does not forward arrow keys used to pick a candidate', () => {
    const { onKeyDown, editor } = setup()
    keyDown(editor, { key: 'ArrowDown', isComposing: true })
    keyDown(editor, { key: 'ArrowUp', isComposing: true })
    expect(onKeyDown).not.toHaveBeenCalled()
  })

  it('forwards Enter once the composition has ended', () => {
    const { onKeyDown, editor } = setup()
    editor.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }))
    keyDown(editor, { key: 'Enter', isComposing: false })
    expect(onKeyDown).toHaveBeenCalledTimes(1)
    expect(onKeyDown.mock.calls[0][0].key).toBe('Enter')
  })

  it('forwards ordinary typing unaffected', () => {
    const { onKeyDown, editor } = setup()
    keyDown(editor, { key: 'a' })
    expect(onKeyDown).toHaveBeenCalledTimes(1)
  })
})
