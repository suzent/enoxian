import { useEffect, useRef, useState } from 'react'
import { PROBLEM_TEXT, type BrokenAgent } from '../lib/agentHealth'
import { openAgentSettings } from '../lib/openAgentSettings'

/**
 * The "⚠ sign-in" badge on a broken agent, and what clicking it does.
 *
 * On this device the fix is in this device's settings, so it opens them at the
 * agent. Anywhere else nothing here can fix it, so it says where and how, with
 * the agent's own error — a badge that looks clickable must never do nothing.
 */
export default function AgentFixBadge({ broken, device, local, leftOut, className }: {
  broken: BrokenAgent
  /** The device's name, as the room knows it. */
  device: string
  /** Whether the agent runs on this device. */
  local: boolean
  /** Whether this device is known to leave the agent out of the room meanwhile. */
  leftOut?: boolean
  className?: string
}) {
  const [open, setOpen] = useState(false)
  const root = useRef<HTMLSpanElement>(null)
  useEffect(() => {
    if (!open) return
    const close = (event: Event) => {
      if (event instanceof KeyboardEvent ? event.key === 'Escape' : !root.current?.contains(event.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', close)
    return () => { document.removeEventListener('mousedown', close); document.removeEventListener('keydown', close) }
  }, [open])

  const agent = `@${broken.agent}`
  const fixesHere = local && (broken.problem === 'sign_in' || broken.problem === 'missing')
  if (fixesHere) {
    return (
      <button type="button" className={`agent-fix ${className ?? ''}`} onClick={() => openAgentSettings(broken.agent)} title={`Open ${agent} in this device's settings${leftOut ? '. Until it works again it is left out of this room.' : ''}\n\n${broken.detail}`}>
        ⚠ {broken.problem === 'sign_in' ? 'Sign in' : 'Fix'} →
      </button>
    )
  }
  const where = local ? 'this device' : device
  const how = {
    sign_in: `Sign ${agent} in again on ${where}: in enox there, open Global settings → Agents and choose Sign in again.`,
    missing: `${agent} can't start on ${where}. Install it there, or turn it off in that device's Global settings → Agents.`,
    limited: `${agent} has used up its allowance on ${where}. It works again once the limit resets, or with another account or model.`,
    failing: `${agent}'s last ${broken.failures} turns on ${where} all failed. Check the agent there, and that enox on ${where} is up to date.`,
  }[broken.problem]
  return (
    <span className="agent-fix-anchor" ref={root}>
      <button type="button" className={`agent-fix ${className ?? ''}`} aria-expanded={open} onClick={() => setOpen(!open)}>
        ⚠ {PROBLEM_TEXT[broken.problem]}
      </button>
      {open && (
        <span className="agent-fix-note" role="dialog" aria-label={`How to fix ${agent}`}>
          <span className="agent-fix-note__how">{how}</span>
          {leftOut && <span>Until then it is left out of this room, and messages nobody addressed go to other agents.</span>}
          <span className="agent-fix-note__count">{broken.failures} failed {broken.failures === 1 ? 'turn' : 'turns'} since it last worked:</span>
          {broken.detail && <span className="agent-fix-note__detail">{broken.detail}</span>}
        </span>
      )}
    </span>
  )
}
