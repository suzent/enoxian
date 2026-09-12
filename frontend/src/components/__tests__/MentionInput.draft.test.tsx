import { describe, it, expect, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import { createRef } from 'react'
import MentionInput, { draftText, type DraftNode, type MentionInputHandle } from '../MentionInput'

/**
 * The composer stays mounted when you switch circles, so an unsent message has
 * to be captured and put back by hand. These cover that round-trip: what comes
 * out of `onChange` must go back in through `restore` unchanged — including
 * committed @mention chips, which would otherwise decay into plain text.
 */
function setup() {
  const onKeyDown = vi.fn()
  const onChange = vi.fn<(text: string, fragment: string | null, nodes: DraftNode[]) => void>()
  const ref = createRef<MentionInputHandle>()
  render(
    <MentionInput ref={ref} placeholder="msg" onChange={onChange} onKeyDown={onKeyDown} />,
  )
  return { onChange, ref, editor: screen.getByRole('textbox') as HTMLElement }
}

/** The nodes from the most recent onChange call. */
function latestNodes(onChange: ReturnType<typeof setup>['onChange']): DraftNode[] {
  return onChange.mock.calls[onChange.mock.calls.length - 1][2]
}

/** Put the caret at the end of the last text node, where a real browser leaves
 *  it after typing — insertMention splits the text node under the caret. */
function caretToEnd(editor: HTMLElement) {
  const last = editor.childNodes[editor.childNodes.length - 1]
  const range = document.createRange()
  range.setStart(last, last.textContent?.length ?? 0)
  range.collapse(true)
  const sel = window.getSelection()!
  sel.removeAllRanges()
  sel.addRange(range)
}

function type(editor: HTMLElement, text: string) {
  editor.appendChild(document.createTextNode(text))
  editor.dispatchEvent(new InputEvent('input', { bubbles: true }))
}

describe('MentionInput draft capture and restore', () => {
  it('reports plain text as a single text node', () => {
    const { onChange, editor } = setup()
    type(editor, 'half a thought')
    const nodes = latestNodes(onChange)
    expect(nodes).toEqual([{ kind: 'text', value: 'half a thought' }])
    expect(draftText(nodes)).toBe('half a thought')
  })

  it('reports a committed mention as a chip node, not as text', () => {
    const { onChange, ref, editor } = setup()
    // insertMention replaces the half-typed `@frag` under the caret, which is
    // how the popup commits a pick.
    type(editor, 'ping @cla')
    caretToEnd(editor)
    ref.current!.insertMention('claude')

    const nodes = latestNodes(onChange)
    expect(nodes).toContainEqual({ kind: 'mention', token: 'claude' })
    // The wire format is unchanged by the richer in-memory representation.
    // The trailing separator is the nbsp insertMention adds after a chip.
    expect(draftText(nodes)).toBe('ping @claude\u00a0')
  })

  it('restores a draft with its chips intact', () => {
    const { ref, editor } = setup()
    const draft: DraftNode[] = [
      { kind: 'text', value: 'ship it ' },
      { kind: 'mention', token: 'codex' },
      { kind: 'text', value: ' today' },
    ]
    ref.current!.restore(draft)

    expect(editor.querySelectorAll('[data-mention]')).toHaveLength(1)
    expect(editor.querySelector('[data-mention]')!.getAttribute('data-mention')).toBe('codex')
    expect(editor.textContent).toBe('ship it @codex today')
  })

  it('restore does not fire onChange — the caller already holds the value', () => {
    const { onChange, ref } = setup()
    onChange.mockClear()
    ref.current!.restore([{ kind: 'text', value: 'quiet' }])
    expect(onChange).not.toHaveBeenCalled()
  })

  it('restoring an empty draft leaves the element empty so the placeholder shows', () => {
    const { ref, editor } = setup()
    type(editor, 'something')
    ref.current!.restore([])
    // `.mention-input:empty::before` renders the placeholder, so a stray empty
    // text node here would silently hide it.
    expect(editor.childNodes).toHaveLength(0)
    expect(editor.matches(':empty')).toBe(true)
  })

  it('round-trips a captured draft back to the same text', () => {
    const { onChange, ref, editor } = setup()
    type(editor, 'unsent words')
    const captured = latestNodes(onChange)

    ref.current!.clear()
    expect(editor.textContent).toBe('')

    ref.current!.restore(captured)
    expect(editor.textContent).toBe('unsent words')
    expect(draftText(captured)).toBe('unsent words')
  })
})
