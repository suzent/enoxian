import { useEffect, useState } from 'react'

/** Keep incomplete input local; only commit a complete number on blur/Enter. */
export default function NumberField({ value, onCommit, min = 0, max, step = 1, disabled, 'aria-label': label, className }: {
  value: number; onCommit: (value: number) => void; min?: number; max?: number; step?: number;
  disabled?: boolean; 'aria-label': string; className?: string
}) {
  const [draft, setDraft] = useState(String(value))
  useEffect(() => setDraft(String(value)), [value])
  const commit = () => {
    if (disabled) return
    if (!draft.trim() || !Number.isFinite(Number(draft))) { setDraft(String(value)); return }
    const next = Math.min(max ?? Infinity, Math.max(min, Math.round(Number(draft))))
    setDraft(String(next))
    if (next !== value) onCommit(next)
  }
  return <input type="number" inputMode="numeric" min={min} max={max} step={step} disabled={disabled}
    aria-label={label} className={className} value={draft} onChange={event => setDraft(event.target.value)}
    onBlur={commit} onKeyDown={event => {
      if (event.nativeEvent.isComposing) return
      if (event.key === 'Enter') { event.preventDefault(); event.currentTarget.blur() }
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setDraft(String(value)) }
    }} />
}
