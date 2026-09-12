import { useRef, useCallback, useImperativeHandle, forwardRef, useEffect } from 'react'

/**
 * A chat input that renders committed @mentions as atomic, non-editable chips
 * (Slack/Discord style): backspace deletes a chip as one unit, and a chip can't
 * be split by typing inside it. Plain text (including a half-typed `@frag`
 * before it is committed) is normal editable text.
 *
 * The component is a thin contentEditable wrapper. It reports two things up:
 *  - `onChange(text, fragment, nodes)` — the plaintext value, the active
 *    `@fragment` the caret is currently in (or null) so the parent can drive the
 *    popup, and the same contents as structured `DraftNode`s.
 *  - `onSend()` / `onKey` — key handling the parent needs (Enter, popup nav).
 *
 * Chips are inserted via the imperative `insertMention(token)` handle, called
 * when the user picks from the popup. Serialization back to plaintext turns each
 * chip into `@token`, so the backend sees exactly the same string as before.
 */

/**
 * A composer's contents in a form that survives being put aside and brought
 * back: chips stay chips instead of decaying into plain `@token` text. Used to
 * keep an unsent message per circle across a circle switch.
 */
export type DraftNode =
  | { kind: 'text'; value: string }
  | { kind: 'mention'; token: string }

export interface MentionInputHandle {
  insertMention: (token: string) => void
  clear: () => void
  focus: () => void
  /** Replace the contents with a previously captured draft. Deliberately does
   *  not fire `onChange`: the caller already holds this value, and re-emitting
   *  it would look like the user typing (and broadcast a typing indicator). */
  restore: (nodes: DraftNode[]) => void
}

interface Props {
  placeholder?: string
  className?: string
  disabled?: boolean
  onChange: (text: string, fragment: string | null, nodes: DraftNode[]) => void
  onKeyDown: (e: React.KeyboardEvent) => void
  /** Given first refusal on paste. If it calls `preventDefault` (as the file
   *  handler does), the plaintext insert below is skipped. */
  onPaste?: (e: React.ClipboardEvent) => void
}

const CHIP_ATTR = 'data-mention'

/** Read the editable DOM into draft nodes. Empty text nodes are dropped so a
 *  round-trip through `restore` leaves the element `:empty` (and therefore
 *  showing its placeholder) when the draft is blank. */
function snapshot(root: HTMLElement): DraftNode[] {
  const out: DraftNode[] = []
  const pushText = (value: string) => { if (value) out.push({ kind: 'text', value }) }
  root.childNodes.forEach(node => {
    if (node.nodeType === Node.TEXT_NODE) {
      pushText(node.textContent ?? '')
    } else if (node instanceof HTMLElement) {
      const token = node.getAttribute(CHIP_ATTR)
      if (token !== null) {
        out.push({ kind: 'mention', token })
      } else if (node.tagName === 'BR') {
        // ignore — Enter is send, not newline
      } else {
        pushText(node.textContent ?? '')
      }
    }
  })
  return out
}

/** Serialize draft nodes to plaintext: chips → `@token`, text as-is. This is
 *  exactly what the backend receives, so a restored draft sends identically. */
export function draftText(nodes: DraftNode[]): string {
  return nodes.map(n => (n.kind === 'mention' ? `@${n.token}` : n.value)).join('')
}

/** Build a chip element for a committed mention. */
function makeChip(token: string): HTMLSpanElement {
  const chip = document.createElement('span')
  chip.setAttribute(CHIP_ATTR, token)
  chip.setAttribute('contenteditable', 'false')
  chip.className = 'mention-chip'
  chip.textContent = `@${token}`
  return chip
}

/** The `@fragment` the caret sits in within a text node, or null. A fragment is
 *  an `@` at the start of the node or after whitespace, up to the caret, with no
 *  whitespace between. */
function activeFragment(): string | null {
  const sel = window.getSelection()
  if (!sel || sel.rangeCount === 0 || !sel.isCollapsed) return null
  const node = sel.anchorNode
  if (!node || node.nodeType !== Node.TEXT_NODE) return null
  const text = node.textContent ?? ''
  const caret = sel.anchorOffset
  const before = text.slice(0, caret)
  const at = before.lastIndexOf('@')
  if (at < 0) return null
  if (at > 0 && !/\s/.test(before[at - 1])) return null
  const frag = before.slice(at + 1)
  if (/\s/.test(frag)) return null
  return frag
}

const MentionInput = forwardRef<MentionInputHandle, Props>(function MentionInput(
  { placeholder, className, disabled = false, onChange, onKeyDown, onPaste },
  ref,
) {
  const elRef = useRef<HTMLDivElement>(null)

  const emit = useCallback(() => {
    const el = elRef.current
    if (!el) return
    const nodes = snapshot(el)
    onChange(draftText(nodes), activeFragment(), nodes)
  }, [onChange])

  useImperativeHandle(ref, () => ({
    focus: () => elRef.current?.focus(),
    clear: () => {
      if (elRef.current) {
        elRef.current.textContent = ''
        emit()
      }
    },
    restore: (nodes: DraftNode[]) => {
      const el = elRef.current
      if (!el) return
      el.textContent = ''
      for (const node of nodes) {
        el.appendChild(
          node.kind === 'mention' ? makeChip(node.token) : document.createTextNode(node.value),
        )
      }
      // Only reposition the caret if the user is actually in this box — a
      // restore triggered by a circle switch must not steal focus.
      if (document.activeElement !== el) return
      const sel = window.getSelection()
      if (!sel) return
      const range = document.createRange()
      range.selectNodeContents(el)
      range.collapse(false)
      sel.removeAllRanges()
      sel.addRange(range)
    },
    insertMention: (token: string) => {
      const el = elRef.current
      if (!el) return
      const sel = window.getSelection()
      if (!sel || sel.rangeCount === 0) return
      const range = sel.getRangeAt(0)
      const node = range.startContainer
      // Replace the half-typed `@frag` in the current text node with a chip.
      if (node.nodeType === Node.TEXT_NODE) {
        const text = node.textContent ?? ''
        const caret = range.startOffset
        const before = text.slice(0, caret)
        const at = before.lastIndexOf('@')
        if (at >= 0) {
          const after = text.slice(caret)
          const head = document.createTextNode(text.slice(0, at))
          const chip = makeChip(token)
          const space = document.createTextNode(' ') // nbsp so caret lands after
          const tail = document.createTextNode(after)
          const parent = node.parentNode!
          parent.replaceChild(tail, node)
          parent.insertBefore(space, tail)
          parent.insertBefore(chip, space)
          parent.insertBefore(head, chip)
          // Caret just after the inserted space.
          const r = document.createRange()
          r.setStart(space, 1)
          r.collapse(true)
          sel.removeAllRanges()
          sel.addRange(r)
        }
      }
      emit()
    },
  }), [emit])

  // Backspace immediately before a chip should delete the whole chip.
  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (disabled) {
      e.preventDefault()
      return
    }
    // While an IME composition is in flight the keys belong to the candidate
    // window, not to us: Backspace is editing the composition rather than
    // deleting a chip, and Enter is accepting a candidate rather than sending.
    // Pass it straight through untouched.
    const native = e.nativeEvent as KeyboardEvent
    if (native.isComposing || native.keyCode === 229) return

    if (e.key === 'Backspace') {
      const sel = window.getSelection()
      if (sel && sel.isCollapsed && sel.rangeCount > 0) {
        const range = sel.getRangeAt(0)
        const node = range.startContainer
        let chipToRemove: Element | null = null
        // Caret at offset 0 of a text node whose previous sibling is a chip,
        // or directly after a chip element.
        if (node.nodeType === Node.TEXT_NODE && range.startOffset === 0) {
          const prev = node.previousSibling
          if (prev instanceof HTMLElement && prev.hasAttribute(CHIP_ATTR)) chipToRemove = prev
        } else if (node.nodeType === Node.TEXT_NODE && range.startOffset === 1 && (node.textContent ?? '')[0] === ' ') {
          const prev = node.previousSibling
          if (prev instanceof HTMLElement && prev.hasAttribute(CHIP_ATTR)) chipToRemove = prev
        }
        if (chipToRemove) {
          e.preventDefault()
          chipToRemove.remove()
          emit()
          return
        }
      }
    }
    onKeyDown(e)
  }

  // Paste as plain text only (no rich HTML sneaking in).
  const handlePaste = (e: React.ClipboardEvent) => {
    if (disabled) {
      e.preventDefault()
      return
    }
    // An image paste is consumed by the parent; inserting the clipboard's text
    // fallback (often the filename) on top of it would be wrong.
    onPaste?.(e)
    if (e.defaultPrevented) return
    e.preventDefault()
    const text = e.clipboardData.getData('text/plain')
    document.execCommand('insertText', false, text)
    emit()
  }

  useEffect(() => { emit() }, [emit])

  return (
    <div
      ref={elRef}
      contentEditable={!disabled}
      role="textbox"
      aria-multiline="false"
      aria-disabled={disabled}
      tabIndex={disabled ? -1 : 0}
      data-placeholder={placeholder}
      className={`mention-input ${className ?? ''}`}
      onInput={emit}
      onKeyDown={handleKeyDown}
      onPaste={handlePaste}
      suppressContentEditableWarning
    />
  )
})

export default MentionInput
