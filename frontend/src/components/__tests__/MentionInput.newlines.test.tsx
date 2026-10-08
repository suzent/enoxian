import { describe, it, expect, vi } from 'vitest'
import { render, screen } from '@testing-library/react'
import MentionInput, { type DraftNode } from '../MentionInput'

/**
 * A browser stores a pasted or Shift+Enter newline as markup, not text: a
 * `<br>`, or a new `<div>` per line. Reading only text nodes glued the lines
 * of a pasted command block together.
 */
function setup() {
  const onChange = vi.fn<(text: string, fragment: string | null, nodes: DraftNode[]) => void>()
  render(<MentionInput placeholder="msg" onChange={onChange} onKeyDown={vi.fn()} />)
  const editor = screen.getByRole('textbox') as HTMLElement
  const latestText = () => onChange.mock.calls[onChange.mock.calls.length - 1][0]
  const input = (html: string) => {
    editor.innerHTML = html
    editor.dispatchEvent(new InputEvent('input', { bubbles: true }))
  }
  return { input, latestText }
}

describe('MentionInput line breaks', () => {
  it('reads <br> as a newline, but not the trailing placeholder <br>', () => {
    const { input, latestText } = setup()
    input('enox --version<br>Select-String -Path x<br>')
    expect(latestText()).toBe('enox --version\nSelect-String -Path x')
  })

  it('reads one <div> per line as separate lines', () => {
    const { input, latestText } = setup()
    input('first<div>second</div><div>third</div>')
    expect(latestText()).toBe('first\nsecond\nthird')
  })

  it('keeps a mention chip inside a line block', () => {
    const { input, latestText } = setup()
    input('hi<div><span data-mention="bob" contenteditable="false">@bob</span> there</div>')
    expect(latestText()).toBe('hi\n@bob there')
  })
})
