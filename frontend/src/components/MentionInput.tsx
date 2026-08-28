import { useRef, useCallback, useImperativeHandle, forwardRef, useEffect } from 'react'

/**
 * A chat input that renders committed @mentions as atomic, non-editable chips
 * (Slack/Discord style): backspace deletes a chip as one unit, and a chip can't
 * be split by typing inside it. Plain text (including a half-typed `@frag`
 * before it is committed) is normal editable text.
 *
 * The component is a thin contentEditable wrapper. It reports two things up:
 *  - `onChange(text, fragment)` — the plaintext value, and the active `@fragment`
 *    the caret is currently in (or null), so the parent can drive the popup.
 *  - `onSend()` / `onKey` — key handling the parent needs (Enter, popup nav).
 *
 * Chips are inserted via the imperative `insertMention(token)` handle, called
 * when the user picks from the popup. Serialization back to plaintext turns each
 * chip into `@token`, so the backend sees exactly the same string as before.
 */

export interface MentionInputHandle {
  insertMention: (token: string) => void
  clear: () => void
  focus: () => void
}

interface Props {
  placeholder?: string
  className?: string
  disabled?: boolean
  onChange: (text: string, fragment: string | null) => void
  onKeyDown: (e: React.KeyboardEvent) => void
}

const CHIP_ATTR = 'data-mention'

/** Serialize the editable DOM to plaintext: chips → `@token`, text as-is. */
function serialize(root: HTMLElement): string {
  let out = ''
  root.childNodes.forEach(node => {
    if (node.nodeType === Node.TEXT_NODE) {
      out += node.textContent ?? ''
    } else if (node instanceof HTMLElement) {
      const token = node.getAttribute(CHIP_ATTR)
      if (token !== null) {
        out += `@${token}`
      } else if (node.tagName === 'BR') {
        // ignore — Enter is send, not newline
      } else {
        out += node.textContent ?? ''
      }
    }
  })
  return out
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
  { placeholder, className, disabled = false, onChange, onKeyDown },
  ref,
) {
  const elRef = useRef<HTMLDivElement>(null)

  const emit = useCallback(() => {
    const el = elRef.current
    if (!el) return
    onChange(serialize(el), activeFragment())
  }, [onChange])

  useImperativeHandle(ref, () => ({
    focus: () => elRef.current?.focus(),
    clear: () => {
      if (elRef.current) {
        elRef.current.textContent = ''
        emit()
      }
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
    e.preventDefault()
    if (disabled) return
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
