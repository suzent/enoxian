import { useMemo } from 'react'
import type { Member, Presence } from '../types'
import { shortenAgentId, peerLabel } from '../lib/displayName'

/**
 * Mention autocomplete over the user → device → agent hierarchy.
 *
 * All three levels are mentionable:
 *   @owner                     — the user (notify)
 *   @owner/device              — a device (notify)
 *   @owner/device/agent        — a device's agent (runs, if that device is on
 *                                push policy)
 *
 * Reachability marks reflect only what this client can know: whether the target
 * device is online and advertises the agent. A remote device's push/pull
 * reaction policy is deliberately not synced, so a green mark means "reachable",
 * not "guaranteed to run".
 */

export interface MentionItem {
  /** The text inserted after `@` (slash-joined). */
  insert: string
  /** 0 = user, 1 = device, 2 = agent — controls indentation. */
  level: 0 | 1 | 2
  /** Display label for this row. */
  label: string
  /** Agent rows only: is the owning device online? */
  online?: boolean
  /** Agent rows: this row launches something if reachable. */
  runnable: boolean
}

interface Props {
  members: Member[]
  presence: Presence[]
  /** The fragment already typed after `@` (may be empty, may contain `/`). */
  fragment: string
  activeIndex: number
  onSelect: (item: MentionItem) => void
  onHover: (index: number) => void
}

/** Build the flattened, filtered mention list from members + presence. */
export function buildMentionItems(
  members: Member[],
  presence: Presence[],
  fragment: string,
): MentionItem[] {
  const onlineByPeer = new Map<string, boolean>()
  const onlineByAgent = new Map<string, boolean>()
  for (const p of presence) {
    const isOnline = p.status !== 'offline'
    if (p.peer_id) onlineByPeer.set(p.peer_id, isOnline)
    onlineByAgent.set(p.agent_id, isOnline)
  }

  // Group members by owner (user), then list devices and their agents.
  const byOwner = new Map<string, Member[]>()
  for (const m of members) {
    const list = byOwner.get(m.owner) ?? []
    list.push(m)
    byOwner.set(m.owner, list)
  }

  const items: MentionItem[] = []
  for (const [owner, devices] of byOwner) {
    const ownerLabel = peerLabel(owner, devices[0]?.agent_id ?? '')
    // Use the owner string as the mention token; fall back to a short id.
    const ownerToken = owner || shortenAgentId(devices[0]?.agent_id ?? '')
    items.push({ insert: ownerToken, level: 0, label: ownerLabel, runnable: false })

    for (const d of devices) {
      const deviceToken = d.device_label || shortenAgentId(d.agent_id)
      const online =
        onlineByPeer.get(d.peer_id) ?? onlineByAgent.get(d.agent_id) ?? false
      items.push({
        insert: `${ownerToken}/${deviceToken}`,
        level: 1,
        label: deviceToken,
        online,
        runnable: false,
      })
      for (const agent of d.agents) {
        items.push({
          insert: `${ownerToken}/${deviceToken}/${agent}`,
          level: 2,
          label: agent,
          online,
          runnable: true,
        })
      }
    }
  }

  const frag = fragment.toLowerCase()
  if (!frag) return items
  // Match against the insert token or the label, so typing "claude" surfaces
  // agent rows and typing "alice/lap" narrows to that device subtree.
  return items.filter(
    it => it.insert.toLowerCase().includes(frag) || it.label.toLowerCase().includes(frag),
  )
}

export default function MentionPopup({
  members,
  presence,
  fragment,
  activeIndex,
  onSelect,
  onHover,
}: Props) {
  const items = useMemo(
    () => buildMentionItems(members, presence, fragment),
    [members, presence, fragment],
  )

  if (items.length === 0) return null

  return (
    <div className="mention-popup sys-window">
      <div className="mention-popup__hint">MENTION — ↑↓ pick · tab insert · ↵ send · esc close</div>
      <div className="mention-popup__list">
        {items.map((it, i) => (
          <button
            key={it.insert + it.level}
            className={`mention-row${i === activeIndex ? ' active' : ''}`}
            style={{ paddingLeft: `${8 + it.level * 14}px` }}
            onMouseEnter={() => onHover(i)}
            onMouseDown={e => { e.preventDefault(); onSelect(it) }}
            title={
              it.level === 2
                ? it.online
                  ? 'Agent — device online & advertises it (reachable; runs if that device is push-policy)'
                  : 'Agent — device offline (won’t run now)'
                : it.level === 1
                  ? 'Device — notify'
                  : 'User — notify'
            }
          >
            <span className="mention-row__prefix">
              {it.level === 0 ? '@' : it.level === 1 ? '└' : '·'}
            </span>
            <span className="mention-row__label">{it.label}</span>
            {it.level === 2 && (
              <span className={`mention-row__mark ${it.online ? 'ok' : 'off'}`}>
                {it.online ? '● runs' : '○ offline'}
              </span>
            )}
            {it.level === 0 && <span className="mention-row__tag">user</span>}
            {it.level === 1 && <span className="mention-row__tag">device</span>}
          </button>
        ))}
      </div>
    </div>
  )
}
