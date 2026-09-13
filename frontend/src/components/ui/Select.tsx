import { useEffect, useId, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Check, ChevronDown } from 'lucide-react'

interface Option { value: string; label: string; group?: string }
interface Props {
  value: string
  options: Option[]
  disabled?: boolean
  'aria-label': string
  onChange: (value: string) => void
}

/** A themed select-only combobox; focus stays on the trigger while navigating. */
export default function Select({ value, options, disabled, onChange, 'aria-label': label }: Props) {
  const id = useId()
  const trigger = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const [position, setPosition] = useState<{ left: number; top: number; width: number; maxHeight: number } | null>(null)
  const [active, setActive] = useState(0)
  const search = useRef({ text: '', time: 0 })
  const selected = Math.max(0, options.findIndex(option => option.value === value))
  const open = () => {
    if (disabled) return
    const rect = trigger.current!.getBoundingClientRect()
    setPosition({ left: rect.left, top: rect.bottom + 4, width: rect.width, maxHeight: Math.max(80, Math.min(280, window.innerHeight - rect.bottom - 16)) })
    setActive(selected)
  }
  const choose = (index: number) => {
    onChange(options[index].value)
    setPosition(null)
    trigger.current?.focus()
  }
  useEffect(() => {
    if (!position) return
    const dismiss = (event: PointerEvent) => {
      if (!trigger.current?.contains(event.target as Node) && !menu.current?.contains(event.target as Node)) setPosition(null)
    }
    const resize = () => setPosition(null)
    document.addEventListener('pointerdown', dismiss)
    window.addEventListener('resize', resize)
    return () => { document.removeEventListener('pointerdown', dismiss); window.removeEventListener('resize', resize) }
  }, [position])
  useEffect(() => { menu.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: 'nearest' }) }, [active, position])
  useEffect(() => { if (disabled) setPosition(null) }, [disabled])
  return (
    <span className="ui-select">
      <button ref={trigger} type="button" role="combobox" aria-label={label}
        aria-expanded={!!position} aria-haspopup="listbox" aria-controls={position ? id : undefined}
        aria-activedescendant={position ? `${id}-${active}` : undefined} disabled={disabled}
        className="ui-select__trigger" onClick={() => position ? setPosition(null) : open()}
        onBlur={() => setPosition(null)}
        onKeyDown={event => {
          if (event.key === 'Tab' || event.key === 'Escape') { setPosition(null); return }
          if (['ArrowDown', 'ArrowUp', 'Home', 'End', 'Enter', ' '].includes(event.key)) {
            event.preventDefault()
            if (!position) { open(); return }
            if (event.key === 'Enter' || event.key === ' ') choose(active)
            else setActive(index => event.key === 'Home' ? 0 : event.key === 'End' ? options.length - 1 : Math.max(0, Math.min(options.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1))))
          } else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
            event.preventDefault()
            if (!position) open()
            const now = Date.now()
            search.current = { text: (now - search.current.time < 700 ? search.current.text : '') + event.key.toLowerCase(), time: now }
            const index = options.findIndex(option => option.label.toLowerCase().startsWith(search.current.text))
            if (index >= 0) setActive(index)
          }
        }}>
        <span>{options[selected]?.label}</span><ChevronDown size={14} aria-hidden="true" />
      </button>
      {position && createPortal(
        <div ref={menu} id={id} role="listbox" aria-label={label} className="ui-select__menu" style={position}
          onMouseDown={event => event.preventDefault()}>
          {options.map((option, index) => (
            <div key={option.value}>
              {option.group && option.group !== options[index - 1]?.group && <div className="ui-select__group" role="presentation">{option.group}</div>}
              <div id={`${id}-${index}`} role="option" aria-selected={option.value === value} data-index={index}
                className={`ui-select__option${index === active ? ' is-active' : ''}`}
                onMouseMove={() => setActive(index)} onClick={() => choose(index)}>
                <Check size={13} aria-hidden="true" style={{ visibility: option.value === value ? 'visible' : 'hidden' }} />
                <span>{option.label}</span>
              </div>
            </div>
          ))}
        </div>, document.body,
      )}
    </span>
  )
}
