import { useState } from 'react'
import { circleFamily } from '../lib/circleIdentity'
import CircleGlyph from './CircleGlyph'

interface Props {
  circleId?: string
  unread?: boolean
  onRead?: () => void
  name: string
  size: number
  working: number
  typing: number
  voided?: boolean
  onOpen: () => void
}

/** A quiet identity mark that becomes a shortcut to real Circle activity. */
export default function CircleActivityButton({ name, circleId=name, unread, onRead, size, working, typing, voided, onOpen }: Props) {
  const family = circleFamily(circleId)
  const [focused, setFocused] = useState(false)
  const active = !voided && (working > 0 || typing > 0)
  const label = voided ? 'DISABLED' : working > 0 ? `${working} WORKING` : typing > 0 ? `${typing} TYPING` : 'Activity'
  return (
    <div className="circle-identity-control">
      <span data-circle-dock aria-hidden="true">
        <CircleGlyph circleId={circleId} family={family} unread={unread} name={name} size={size} voided={voided} active={active} engaged={focused} />
      </span>
      <span className="circle-activity-button__name" title={name}>{name}</span>
      <button
      type="button"
      className={`circle-activity-button${active ? ' is-active' : ''}`}
      onClick={() => { onRead?.(); onOpen() }}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      aria-label={`View activity for ${name}`}
      title="Open agent activity"
    >
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><path d="M2 12h5l3-7 4 14 3-7h5" /></svg>
      <span className="circle-activity-button__label">{label}</span>
    </button>
    <span className="sr-only" role="status">{unread ? 'New messages' : ''}</span>
    </div>
  )
}
