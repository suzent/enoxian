import { useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'

export interface MenuAnchor { x: number; y: number; trigger: HTMLElement }
export interface MenuAction { label: string; icon?: ReactNode; danger?: boolean; onSelect: () => void }

/** One menu surface for pointer, keyboard and overflow-button entry points. */
export default function ActionMenu({ anchor, actions, onClose, label }: {
  anchor: MenuAnchor; actions: MenuAction[]; onClose: () => void; label: string
}) {
  const ref = useRef<HTMLDivElement>(null)
  const close = useRef(onClose)
  close.current = onClose
  const [position, setPosition] = useState({ left: anchor.x, top: anchor.y })
  useLayoutEffect(() => {
    const menu = ref.current!
    const rect = menu.getBoundingClientRect()
    setPosition({ left: Math.max(8, Math.min(anchor.x, innerWidth - rect.width - 8)), top: Math.max(8, Math.min(anchor.y, innerHeight - rect.height - 8)) })
    menu.querySelector('button')?.focus()
    const dismiss = (event: PointerEvent) => {
      if (!menu.contains(event.target as Node) && !anchor.trigger.contains(event.target as Node)) close.current()
    }
    const resize = () => close.current()
    document.addEventListener('pointerdown', dismiss)
    window.addEventListener('resize', resize)
    return () => {
      document.removeEventListener('pointerdown', dismiss)
      window.removeEventListener('resize', resize)
      if (menu.contains(document.activeElement) && anchor.trigger.isConnected) anchor.trigger.focus()
    }
  }, [anchor])
  return createPortal(<div ref={ref} role="menu" aria-label={label} className="action-menu" style={position}
    onContextMenu={event => event.preventDefault()}
    onKeyDown={event => {
      const items = [...ref.current!.querySelectorAll<HTMLButtonElement>('button')]
      const index = items.indexOf(document.activeElement as HTMLButtonElement)
      if (event.key === 'Escape' || event.key === 'Tab') {
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); anchor.trigger.focus() }
        onClose()
      } else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
        event.preventDefault()
        const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length
        items[next]?.focus()
      }
    }}>
    {actions.map(action => <button key={action.label} type="button" role="menuitem" className={action.danger ? 'is-danger' : ''}
      onClick={() => { onClose(); action.onSelect() }}>{action.icon}<span>{action.label}</span></button>)}
  </div>, document.body)
}
