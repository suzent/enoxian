import { useCallback, useEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import type { Attachment } from '../types'
import { blobUrl } from '../api'

interface Props {
  circleId: string
  /** Every image in the transcript, so the viewer can page through them. */
  items: Attachment[]
  /** Index into `items` of the image being shown. */
  index: number
  onIndexChange: (index: number) => void
  onClose: () => void
}

/**
 * Full-size image viewer.
 *
 * Opening a browser tab for each image loses the reader's place in the
 * transcript and drops them onto a bare blob URL with no context. This keeps
 * them in the app, and lets them page through every image in the conversation.
 *
 * Modal behaviour that has to be right or the dialog traps people: Escape and
 * backdrop clicks close it, focus moves in and is restored on close, focus is
 * trapped while open, and background scrolling is frozen.
 *
 * Rendered through a portal to `document.body` rather than in place. The chat
 * panel sets `overflow: hidden` and paints a white background on its direct
 * children (`.app-chat-panel > div`), both of which would break a fixed-position
 * overlay nested inside it.
 */
export default function Lightbox({ circleId, items, index, onIndexChange, onClose }: Props) {
  const dialogRef = useRef<HTMLDivElement>(null)
  const closeRef = useRef<HTMLButtonElement>(null)
  // Where focus was before we opened, so it can be handed back on close.
  const restoreRef = useRef<Element | null>(null)

  const current = items[index]
  const hasPrev = index > 0
  const hasNext = index < items.length - 1

  const go = useCallback(
    (delta: number) => {
      const next = index + delta
      if (next >= 0 && next < items.length) onIndexChange(next)
    },
    [index, items.length, onIndexChange],
  )

  useEffect(() => {
    restoreRef.current = document.activeElement
    closeRef.current?.focus()
    const { overflow } = document.body.style
    document.body.style.overflow = 'hidden'
    return () => {
      document.body.style.overflow = overflow
      // Only restore if focus is still somewhere inside the closing dialog —
      // otherwise the user has already clicked elsewhere and we would yank it.
      if (
        restoreRef.current instanceof HTMLElement &&
        (!document.activeElement || dialogRef.current?.contains(document.activeElement))
      ) {
        restoreRef.current.focus()
      }
    }
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
        return
      }
      if (e.key === 'ArrowLeft') {
        e.preventDefault()
        go(-1)
        return
      }
      if (e.key === 'ArrowRight') {
        e.preventDefault()
        go(1)
        return
      }
      if (e.key !== 'Tab') return
      // Trap focus: with a handful of controls, cycling manually is simpler
      // and more predictable than tracking the document's tab order.
      const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), a[href]',
      )
      if (!focusable || focusable.length === 0) return
      const first = focusable[0]
      const last = focusable[focusable.length - 1]
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault()
        last.focus()
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault()
        first.focus()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [go, onClose])

  if (!current) return null

  return createPortal(
    <div
      ref={dialogRef}
      className="lightbox"
      role="dialog"
      aria-modal="true"
      aria-label={`${current.name} — image ${index + 1} of ${items.length}`}
      // Backdrop click closes, but only when the backdrop itself was hit:
      // without this check, releasing a drag over the backdrop after selecting
      // inside the image would close the viewer.
      onMouseDown={e => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="lightbox__bar">
        <span className="lightbox__name" title={current.name}>
          {current.name}
          {items.length > 1 && (
            <span className="lightbox__count">
              {' '}
              {index + 1}/{items.length}
            </span>
          )}
        </span>
        <a
          className="lightbox__btn"
          href={blobUrl(circleId, current.hash)}
          download={current.name}
          onClick={e => e.stopPropagation()}
        >
          DOWNLOAD
        </a>
        <button ref={closeRef} className="lightbox__btn" onClick={onClose} aria-label="Close viewer">
          CLOSE ✕
        </button>
      </div>

      <div className="lightbox__stage">
        {items.length > 1 && (
          <button
            className="lightbox__nav lightbox__nav--prev"
            onClick={() => go(-1)}
            disabled={!hasPrev}
            aria-label="Previous image"
          >
            ‹
          </button>
        )}
        <img
          className="lightbox__img"
          // Keyed by hash so paging swaps the element rather than showing the
          // previous image stretched to the new one's dimensions.
          key={current.hash}
          src={blobUrl(circleId, current.hash)}
          alt={current.name}
          width={current.width}
          height={current.height}
        />
        {items.length > 1 && (
          <button
            className="lightbox__nav lightbox__nav--next"
            onClick={() => go(1)}
            disabled={!hasNext}
            aria-label="Next image"
          >
            ›
          </button>
        )}
      </div>
    </div>,
    document.body,
  )
}
