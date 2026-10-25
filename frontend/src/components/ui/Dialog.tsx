import { useEffect, useId, useRef, type ReactNode } from 'react'
import { createPortal } from 'react-dom'

const stack: symbol[] = []
let rootBefore: { element: HTMLElement; inert: boolean } | null = null
const focusable = 'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex="0"]'

/** Shared modal behavior, including nested confirmations and focus restoration. */
export default function Dialog({ title, children, onClose, className = '' }: {
  title: string; children: ReactNode; onClose: () => void; className?: string
}) {
  const panel = useRef<HTMLDivElement>(null)
  const close = useRef(onClose)
  close.current = onClose
  const titleId = useId()
  useEffect(() => {
    const entry = Symbol()
    if (stack.length === 0) {
      const root = document.getElementById('root')
      if (root) { rootBefore = { element: root, inert: root.inert }; root.inert = true }
    }
    stack.push(entry)
    // Portaled menus stay interactive; the app and any underlying dialog do not.
    const background = [...document.querySelectorAll<HTMLElement>('[role="dialog"]')]
      .filter((el): el is HTMLElement => !!el && el !== panel.current && !el.contains(panel.current))
    const inertBefore = background.map(el => el.inert)
    background.forEach(el => { el.inert = true })
    const previous = document.activeElement as HTMLElement | null
    panel.current?.focus()
    const keydown = (event: KeyboardEvent) => {
      if (stack[stack.length - 1] !== entry || event.defaultPrevented) return
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        close.current()
      }
      if (event.key === 'Tab') {
        const items = Array.from(panel.current?.querySelectorAll<HTMLElement>(focusable) ?? [])
          .filter(el => !el.closest('[hidden]') && el.getClientRects().length > 0)
        const first = items[0], last = items[items.length - 1]
        if (!first) { event.preventDefault(); panel.current?.focus(); return }
        if (event.shiftKey && (document.activeElement === first || document.activeElement === panel.current)) {
          event.preventDefault(); last?.focus()
        } else if (!event.shiftKey && (document.activeElement === last || !panel.current?.contains(document.activeElement))) {
          event.preventDefault(); first.focus()
        }
      }
    }
    document.addEventListener('keydown', keydown)
    return () => {
      stack.splice(stack.indexOf(entry), 1)
      document.removeEventListener('keydown', keydown)
      if (stack.length === 0 && rootBefore) { rootBefore.element.inert = rootBefore.inert; rootBefore = null }
      background.forEach((el, index) => { el.inert = inertBefore[index] })
      if (previous?.isConnected) previous.focus()
    }
  }, [])
  return createPortal(
    <div className="ritual-modal-backdrop" onClick={event => { if (event.target === event.currentTarget) onClose() }}>
      <div ref={panel} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}
        className={`ritual-panel sys-window ${className}`}>
        <button type="button" onClick={onClose} className="ritual-panel__close" aria-label="Close">×</button>
        <div id={titleId} className="ritual-panel__header">{title}</div>
        {children}
      </div>
    </div>, document.body,
  )
}

export function ConfirmDialog({ title, children, onConfirm, onClose, confirmLabel = 'Continue' }: {
  title: string; children: ReactNode; onConfirm: () => void; onClose: () => void; confirmLabel?: string
}) {
  return <Dialog title={title} onClose={onClose} className="settings-confirm">
    <div className="ritual-panel__body">
      <div className="settings-confirm__copy">{children}</div>
      <div className="ritual-actions">
        <button type="button" className="ritual-btn ritual-btn--secondary" onClick={onClose}>Cancel</button>
        <button type="button" className="ritual-btn ritual-btn--primary" onClick={onConfirm}>{confirmLabel}</button>
      </div>
    </div>
  </Dialog>
}
