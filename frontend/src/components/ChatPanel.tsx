import { Fragment, useState, useEffect, useRef, useCallback } from 'react'
import type { ChatMessage, Member, Presence } from '../types'
import { getChat, postChat, chatStream, getMembers, getWho } from '../api'
import { useApp } from '../context/AppContext'
import { shortenAgentId, peerLabel } from '../lib/displayName'
import CircleGlyph from './CircleGlyph'
import MentionPopup, { buildMentionItems, type MentionItem } from './MentionPopup'
import MentionInput, { type MentionInputHandle } from './MentionInput'

interface Props {
  onMessage?: () => void
  variant?: 'rail' | 'main'
  hideActiveCircleGlyph?: boolean
}

/**
 * Render message text with recognized @mentions as chips. `mentions` is the
 * server-parsed list (owner/device/agent bodies) — the ground truth for "this
 * registered as a mention". A `@token` in the text is chipped only if it is in
 * that list, so a typo that matched nothing stays plain text.
 */
function renderWithMentions(text: string, mentions: string[]): React.ReactNode {
  if (!mentions || mentions.length === 0) return text
  // Match @ followed by mention-body chars (letters, digits, -, _, /).
  const parts: React.ReactNode[] = []
  const re = /@([A-Za-z0-9_\-/]+)/g
  let last = 0
  let m: RegExpExecArray | null
  let key = 0
  while ((m = re.exec(text)) !== null) {
    const body = m[1]
    if (!mentions.includes(body)) continue // unrecognized — leave as plain text
    if (m.index > last) parts.push(text.slice(last, m.index))
    parts.push(
      <span key={key++} className="mention-chip mention-chip--msg">@{body}</span>,
    )
    last = m.index + m[0].length
  }
  if (last === 0) return text // nothing chipped
  if (last < text.length) parts.push(text.slice(last))
  return parts
}

function formatTime(ts: number) {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

function formatFullTimestamp(ts: number) {
  return new Date(ts * 1000).toLocaleString([], {
    weekday: 'short',
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function calendarDay(ts: number) {
  const date = new Date(ts * 1000)
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`
}

function formatDateLabel(ts: number) {
  const date = new Date(ts * 1000)
  const today = new Date()
  const yesterday = new Date(today)
  yesterday.setDate(today.getDate() - 1)

  if (calendarDay(ts) === calendarDay(today.getTime() / 1000)) return 'Today'
  if (calendarDay(ts) === calendarDay(yesterday.getTime() / 1000)) return 'Yesterday'

  return date.toLocaleDateString([], {
    weekday: 'long',
    month: 'short',
    day: 'numeric',
    ...(date.getFullYear() !== today.getFullYear() ? { year: 'numeric' as const } : {}),
  })
}

interface SenderLabel {
  user: string    // "you" or owner name
  device: string | null  // shown when owner has multiple devices
  agent: string | null   // shown when a registered agent (not device primary) is speaking
}

function senderInitial(label: SenderLabel) {
  const source = label.agent || label.device || label.user
  return source.match(/[\p{L}\p{N}]/u)?.[0]?.toUpperCase() || '·'
}

interface BubbleProps {
  msg: ChatMessage
  isMine: boolean  // true for all devices owned by self
  isThisDevice: boolean  // true only for the current device
  label: SenderLabel
  showSender: boolean
}

function Bubble({ msg, isMine, isThisDevice, label, showSender }: BubbleProps) {
  if (msg.agent_id === 'system') {
    return (
      <div className="chat-system-event" role="status">
        <span>SYS</span>
        <span>{msg.text}</span>
      </div>
    )
  }

  const isAgent = !!label.agent
  const timestamp = formatTime(msg.ts)
  const fullTimestamp = formatFullTimestamp(msg.ts)

  return (
    <article
      className={`chat-message${isMine ? ' chat-message--mine' : ''}${isThisDevice ? ' chat-message--current' : ''}${isAgent ? ' chat-message--agent' : ''}${showSender ? '' : ' chat-message--continued'}`}
    >
      <div className="chat-message__gutter" aria-hidden="true">
        {showSender
          ? <span className="chat-message__avatar">{senderInitial(label)}</span>
          : <time className="chat-message__gutter-time" dateTime={new Date(msg.ts * 1000).toISOString()} title={fullTimestamp}>{timestamp}</time>}
      </div>
      <div className="chat-message__body">
        {showSender && (
          <header className="chat-message__sender">
            <span className="chat-message__owner">{label.user}</span>
            {label.device && <span className="chat-message__device">· {label.device}</span>}
            {label.agent && <span className="chat-message__agent">AGENT / {label.agent}</span>}
            <time className="chat-message__time" dateTime={new Date(msg.ts * 1000).toISOString()} title={fullTimestamp} aria-label={fullTimestamp}>
              {timestamp}
            </time>
          </header>
        )}
        <div className="chat-message__text">
          {renderWithMentions(msg.text, msg.mentions)}
        </div>
      </div>
    </article>
  )
}

export default function ChatPanel({ onMessage, variant = 'rail', hideActiveCircleGlyph = false }: Props) {
  const { activeCircleId, circles, status } = useApp()
  const activeCircle = circles.find(c => c.circle_id === activeCircleId)
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [members, setMembers] = useState<Member[]>([])
  const [presence, setPresence] = useState<Presence[]>([])
  // Plaintext value of the input, mirrored from MentionInput for send.
  const [input, setInput] = useState('')
  // The active `@fragment` under the caret (drives the popup), or null.
  const [fragment, setFragment] = useState<string | null>(null)
  const [mentionIndex, setMentionIndex] = useState(0)
  // True once the user has navigated the popup with arrows — only then does
  // Enter accept a suggestion instead of sending the message.
  const [mentionActive, setMentionActive] = useState(false)
  const inputRef = useRef<MentionInputHandle>(null)
  const bottomRef = useRef<HTMLDivElement>(null)
  const seenRef = useRef(new Set<string>())
  const latestTsRef = useRef<number | null>(null)

  const addMsg = useCallback((msg: ChatMessage) => {
    if (seenRef.current.has(msg.id)) return
    seenRef.current.add(msg.id)
    latestTsRef.current = Math.max(latestTsRef.current ?? msg.ts, msg.ts)
    setMessages(prev => [...prev, msg].sort((a, b) => a.ts - b.ts || a.id.localeCompare(b.id)))
    onMessage?.()
  }, [onMessage])

  useEffect(() => {
    if (!activeCircleId) return
    let cancelled = false
    seenRef.current.clear()
    latestTsRef.current = null
    setMessages([])
    setMembers([])
    setPresence([])

    const refreshRoster = () => {
      getMembers(activeCircleId).then(m => { if (!cancelled) setMembers(m) }).catch(() => {})
      getWho(activeCircleId).then(p => { if (!cancelled) setPresence(p) }).catch(() => {})
    }
    refreshRoster()

    const catchUp = () => {
      const since = latestTsRef.current === null ? undefined : Math.max(0, latestTsRef.current - 1)
      getChat(activeCircleId, since)
        .then(msgs => { if (cancelled) return; msgs.forEach(addMsg) })
        .catch(() => {})
    }

    const es = chatStream(activeCircleId)
    es.onopen = catchUp
    es.addEventListener('message', e => {
      try {
        const data = JSON.parse(e.data)
        if (data.type === 'message_posted') addMsg(data.message)
        if (data.type === 'member_added' || data.type === 'member_removed' || data.type === 'presence_changed') {
          refreshRoster()
        }
      } catch {}
    })
    catchUp()
    return () => {
      cancelled = true
      es.close()
    }
  }, [activeCircleId, addMsg])

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  // My owner name — used to recognise all my devices as "you"
  const myMember = members.find(m => m.agent_id === status?.agent_id)
  const selfOwner = myMember
    ? peerLabel(myMember.owner, myMember.agent_id)
    : shortenAgentId(status?.agent_id ?? '')

  const getSenderLabel = useCallback((agentId: string): SenderLabel => {
    // Direct member lookup by device agent_id
    let member = members.find(m => m.agent_id === agentId)
    let agentName: string | null = null

    // If not found as a primary device, check registered agent lists (e.g. "claude-code")
    if (!member) {
      const host = members.find(m => m.agents.includes(agentId))
      if (host) { member = host; agentName = agentId }
    }

    if (!member) return { user: shortenAgentId(agentId) || agentId, device: null, agent: null }

    const ownerLabel = peerLabel(member.owner, member.agent_id)
    const isOwnDevice = ownerLabel === selfOwner
    const deviceLabel = member.device_label || shortenAgentId(member.agent_id)

    // Show device qualifier when this owner has more than one device in the circle
    const sameOwnerDevices = members.filter(m => peerLabel(m.owner, m.agent_id) === ownerLabel)
    const showDevice = sameOwnerDevices.length > 1 && !!deviceLabel && deviceLabel !== ownerLabel

    return {
      user: isOwnDevice ? 'you' : ownerLabel,
      device: showDevice ? deviceLabel : null,
      agent: agentName,
    }
  }, [members, selfOwner])

  const send = () => {
    const text = input.trim()
    if (!text || !activeCircleId || !status || activeCircle?.disabled) return
    inputRef.current?.clear()
    setInput('')
    setFragment(null)
    setMentionActive(false)
    postChat(activeCircleId, text, status.agent_id).catch(() => {})
  }

  const mentionOpen = fragment !== null

  // MentionInput reports the plaintext value and the active @fragment together.
  const onInputChange = (text: string, frag: string | null) => {
    setInput(text)
    setFragment(frag)
    // Typing changes the filter — reset navigation so Enter sends until the
    // user explicitly arrows into the list again.
    setMentionActive(false)
    setMentionIndex(0)
  }

  const applyMention = (item: MentionItem) => {
    inputRef.current?.insertMention(item.insert)
    setFragment(null)
    setMentionActive(false)
  }

  const onInputKeyDown = (e: React.KeyboardEvent) => {
    if (mentionOpen) {
      const items = buildMentionItems(members, presence, fragment ?? '')
      if (items.length > 0) {
        if (e.key === 'ArrowDown') {
          e.preventDefault()
          setMentionActive(true)
          setMentionIndex(i => (mentionActive ? (i + 1) % items.length : 0))
          return
        }
        if (e.key === 'ArrowUp') {
          e.preventDefault()
          setMentionActive(true)
          setMentionIndex(i => (mentionActive ? (i - 1 + items.length) % items.length : items.length - 1))
          return
        }
        // Tab always accepts the highlighted suggestion.
        if (e.key === 'Tab') {
          e.preventDefault()
          applyMention(items[Math.min(mentionIndex, items.length - 1)])
          return
        }
        // Enter accepts a suggestion ONLY if the user has navigated the popup
        // with the arrow keys. Otherwise Enter sends the message as typed — so
        // "@claude do it" + Enter posts, it doesn't silently autocomplete.
        if (e.key === 'Enter' && mentionActive) {
          e.preventDefault()
          applyMention(items[Math.min(mentionIndex, items.length - 1)])
          return
        }
        if (e.key === 'Escape') {
          e.preventDefault()
          setFragment(null)
          return
        }
      }
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      send()
    }
  }

  const hasConversation = messages.length > 0
  const onlineCount = presence.filter(p => p.status === 'online').length
  const glyphSize = hasConversation ? 64 : 88

  return (
    <main className={`app-chat-panel flex min-h-0 flex-col z-10 overflow-hidden ${variant === 'main' ? 'chat-main sys-window' : 'border-r-2 border-obsidian bg-alabaster/85'}`}>
      {variant !== 'main' && (
        <div className="section-header">
          <span>Terminal Log</span>
        </div>
      )}

      {variant === 'main' && activeCircle && (
        <div
          className={`active-circle-dock${hasConversation ? ' active-circle-dock--compact' : ' active-circle-dock--empty'}${activeCircle.disabled ? ' active-circle-dock--void' : ''}${hideActiveCircleGlyph ? ' active-circle-dock--ritual' : ''}`}
        >
          <div className="active-circle-dock__meta">
            <span>{activeCircle.circle_name}</span>
            <strong>{activeCircle.disabled ? 'VOID CIRCLE' : 'ACTIVE CIRCLE'}</strong>
          </div>
          <div className="ripple-container" style={{ width: glyphSize, height: glyphSize }}>
            <div className="dock-ripple" id="dock-ripple-el" />
            <div data-circle-dock style={{ width: glyphSize, height: glyphSize }}>
              <CircleGlyph
                name={activeCircle.circle_name}
                size={glyphSize}
                className="active-circle-dock__glyph"
                title={activeCircle.circle_name}
                voided={activeCircle.disabled}
              />
            </div>
          </div>
          <div className="active-circle-dock__meta active-circle-dock__meta--stats">
            <span>{onlineCount} ONLINE</span>
            <strong>{members.length} MEMBERS</strong>
          </div>
        </div>
      )}

      {variant === 'main' && activeCircle?.disabled && (
        <div className="chat-void-notice" role="status">
          <span>VOID MODE</span>
          <strong>THIS CIRCLE IS DISABLED · ENABLE IT TO RESUME</strong>
        </div>
      )}

      <div className="chat-message-list">
        {messages.length === 0 && (
          <div className="chat-empty-state">
            <span>THE CIRCLE IS QUIET</span>
            <strong>Send the first signal.</strong>
          </div>
        )}
        {messages.map((msg, i) => {
          const label = getSenderLabel(msg.agent_id)
          const isThisDevice = msg.agent_id === status?.agent_id
          const isMine = label.user === 'you'
          const prev = messages[i - 1]
          const startsNewDay = !prev || calendarDay(prev.ts) !== calendarDay(msg.ts)
          const showSender = !prev || prev.agent_id !== msg.agent_id || msg.ts - prev.ts > 300
          return (
            <Fragment key={msg.id}>
              {startsNewDay && (
                <div className="chat-date-separator" role="separator" aria-label={formatDateLabel(msg.ts)}>
                  <span>{formatDateLabel(msg.ts)}</span>
                </div>
              )}
              <Bubble
                msg={msg}
                isMine={isMine}
                isThisDevice={isThisDevice}
                label={label}
                showSender={showSender || startsNewDay}
              />
            </Fragment>
          )
        })}
        <div ref={bottomRef} />
      </div>

      <div className="chat-composer">
        {mentionOpen && (
          <MentionPopup
            members={members}
            presence={presence}
            fragment={fragment ?? ''}
            activeIndex={mentionIndex}
            onSelect={applyMention}
            onHover={setMentionIndex}
          />
        )}
        <MentionInput
          ref={inputRef}
          onChange={onInputChange}
          onKeyDown={onInputKeyDown}
          placeholder={activeCircle?.disabled ? 'Circle disabled — enable to resume' : 'Message the circle...  (@ to mention)'}
          className={`chat-composer__input${activeCircle?.disabled ? ' chat-composer__input--disabled' : ''}`}
          disabled={activeCircle?.disabled}
        />
        <button onClick={send} disabled={activeCircle?.disabled} className="enox-btn chat-composer__send">
          {activeCircle?.disabled ? 'VOID' : variant === 'main' ? 'SEND' : 'EXEC'}
        </button>
      </div>
    </main>
  )
}
