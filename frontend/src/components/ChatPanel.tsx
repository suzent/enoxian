import { createPortal } from 'react-dom'
import ExecutionStatus from './ExecutionStatus'
import { Fragment, useState, useEffect, useRef, useCallback, useMemo } from 'react'
import type { Attachment, ChatActivity, ChatMessage, EngagementView, Member, Presence } from '../types'
import { getChat, postChat, chatStream, getChatActivity, setChatTyping, getMembers, getWho, uploadAttachment, blobUrl, stopRelay, getEngagement, exitEngagement, MAX_ATTACHMENT_BYTES } from '../api'
import { useApp } from '../context/AppContext'
import { shortenAgentId, peerLabel } from '../lib/displayName'
import CircleGlyph from './CircleGlyph'
import MentionPopup, { buildMentionItems, type MentionItem } from './MentionPopup'
import MentionInput, { draftText, type DraftNode, type MentionInputHandle } from './MentionInput'
import Lightbox from './Lightbox'
import { renderChatMarkdown } from '../lib/markdown'

// Backfill retry budget: ~0.5s + 1s + 1.5s + 2s before giving up and telling
// the user, rather than rendering an empty transcript as if it were loaded.
const CATCH_UP_MAX_ATTEMPTS = 5
const CATCH_UP_BACKOFF_MS = 500

interface Props {
  onActivityNavigate?: () => void
  activityContainer?: HTMLDivElement | null
  onMessage?: () => void
  variant?: 'rail' | 'main'
  hideActiveCircleGlyph?: boolean
}

/** A message body. Markdown, because agents post structured output — lists,
 *  code blocks, tables — and a wall of raw syntax is unreadable in a bubble.
 *
 *  Parsing is memoised: the transcript re-renders on every incoming message,
 *  presence tick and typing update, and re-parsing every bubble each time makes
 *  a long scrollback crawl.
 */
function MessageText({ text, mentions }: { text: string; mentions: string[] }) {
  const html = useMemo(() => renderChatMarkdown(text, mentions), [text, mentions])
  return (
    <div
      className="chat-message__text markdown-preview markdown-preview--chat"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  )
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

/** The agent that delegated to the one posting this message, if any.
 *
 *  Provenance has to be visible: a reply nobody asked for reads as the Circle
 *  talking to itself unless you can see who asked. */
function relayDelegator(msg: ChatMessage): string | null {
  const path = msg.relay?.path
  if (!path || path.length < 2) return null
  return path[path.length - 2] ?? null
}

/** What to tell the user when a send fails.
 *
 *  The daemon's own wording is better than a generic line whenever it has one
 *  — "Circle state is busy syncing" tells the user to retry; "Message failed
 *  to send" implies something is broken. */
function sendErrorText(error: unknown): string {
  const detail = error instanceof Error ? error.message.trim() : ''
  return detail ? `Not sent — ${detail}` : 'Message failed to send.'
}

function senderInitial(label: SenderLabel) {
  const source = label.agent || label.device || label.user
  return source.match(/[\p{L}\p{N}]/u)?.[0]?.toUpperCase() || '·'
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

interface AttachmentImageProps {
  circleId: string
  att: Attachment
  /** Opens the in-app viewer. Plain clicks only — modified clicks fall through
   *  to the href so "open in new tab" still works. */
  onOpen: () => void
  /** Bumped when the daemon reports this blob finished downloading, which
   *  re-requests a URL that previously 404'd. */
  nonce: number
}

/** One image in the transcript.
 *
 *  The bytes may not have arrived yet — a peer posts the message through the
 *  CRDT immediately, while the blob travels separately over the sync stream.
 *  So a miss is an expected transient state, not an error: we show a
 *  placeholder sized from the stored dimensions and swap in the image when the
 *  `attachment_available` event bumps `nonce`. */
function AttachmentImage({ circleId, att, nonce, onOpen }: AttachmentImageProps) {
  const [failed, setFailed] = useState(false)
  useEffect(() => { setFailed(false) }, [nonce])

  // Reserve the real aspect ratio up front so arriving images don't reflow the
  // transcript under the reader.
  const ratio = att.width && att.height ? `${att.width} / ${att.height}` : undefined

  if (failed) {
    return (
      <div className="chat-attachment chat-attachment--pending" style={{ aspectRatio: ratio }} role="img" aria-label={`${att.name} — not downloaded yet`}>
        <span className="chat-attachment__pending-label">
          Waiting for image…
          <small>{att.name} · {formatBytes(att.size)}</small>
        </span>
      </div>
    )
  }

  return (
    <a
      className="chat-attachment"
      href={blobUrl(circleId, att.hash)}
      target="_blank"
      rel="noreferrer noopener"
      style={{ aspectRatio: ratio }}
      onClick={e => {
        // Let cmd/ctrl/shift/middle-click do what the browser normally does.
        if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return
        e.preventDefault()
        onOpen()
      }}
    >
      <img
        src={`${blobUrl(circleId, att.hash)}&v=${nonce}`}
        alt={att.name}
        width={att.width}
        height={att.height}
        loading="lazy"
        decoding="async"
        onError={() => setFailed(true)}
      />
    </a>
  )
}

interface BubbleProps {
  msg: ChatMessage
  isMine: boolean  // true for all devices owned by self
  isThisDevice: boolean  // true only for the current device
  label: SenderLabel
  showSender: boolean
  circleId: string
  blobNonces: Record<string, number>
  onOpenImage: (hash: string) => void
  replyParent?: ChatMessage
  onReply?: (msg: ChatMessage) => void
}

function Bubble({ msg, isMine, isThisDevice, label, showSender, circleId, blobNonces, onOpenImage, onReply, replyParent }: BubbleProps) {
  if (msg.agent_id === 'system') {
    return (
      <div className="chat-system-event" role="status">
        <span>SYS</span>
        <span>{msg.text}</span>
      </div>
    )
  }

  const isAgent = !!label.agent
  // Who handed this agent the job, if anyone. The relay path ends with the
  // agent that posted, so the one before it is the delegator. A turn a person
  // asked for directly has nothing before it, and shows no chip.
  const delegatedBy = relayDelegator(msg)
  const timestamp = formatTime(msg.ts)
  const fullTimestamp = formatFullTimestamp(msg.ts)

  return (
    <article
      id={`chat-message-${msg.id}`}
      className={`chat-message${msg.reply_to ? ' chat-message--reply' : ''}${isMine ? ' chat-message--mine' : ''}${isThisDevice ? ' chat-message--current' : ''}${isAgent ? ' chat-message--agent' : ''}${showSender ? '' : ' chat-message--continued'}`}
    >
      <div className="chat-message__gutter" aria-hidden="true">
        {showSender
          ? <span className="chat-message__avatar">{senderInitial(label)}</span>
          : <time className="chat-message__gutter-time" dateTime={new Date(msg.ts * 1000).toISOString()} title={fullTimestamp}>{timestamp}</time>}
      </div>
      <div className="chat-message__body">
        {msg.reply_to && (
          <a className="chat-message__parent" href={`#chat-message-${msg.reply_to}`}>
            <span className="chat-message__parent-label"><span aria-hidden="true">↳</span> Replying to</span>
            <span className="chat-message__parent-preview">{replyParent?.text.trim() || 'Waiting for the referenced message to sync'}</span>
          </a>
        )}
        {showSender && (
          <header className="chat-message__sender">
            <span className="chat-message__owner" title={label.agent ? `Owned by ${label.user}` : undefined}>{label.agent || label.user}</span>
            {label.device && <span className="chat-message__device">· {label.device}</span>}
            {label.agent && <span className="chat-message__agent">AGENT</span>}
            {delegatedBy && (
              <span className="chat-message__via" title={`Delegated by @${delegatedBy}. Nobody typed this request.`}>
                via @{delegatedBy}
              </span>
            )}
            <time className="chat-message__time" dateTime={new Date(msg.ts * 1000).toISOString()} title={fullTimestamp} aria-label={fullTimestamp}>
              {timestamp}
            </time>
            {isAgent && onReply && (
              // Explicit addressing: pointing at a reply beats guessing from
              // recency, and is the only thing that works when two agents are
              // mid-conversation with you.
              <button
                type="button"
                className="chat-message__reply"
                onClick={() => onReply(msg)}
                title={`Reply to @${label.agent} — routes there with no mention`}
              >
                reply
              </button>
            )}
          </header>
        )}
        {msg.text.trim() && <MessageText text={msg.text} mentions={msg.mentions} />}
        {!!msg.attachments?.length && (
          <div className="chat-message__attachments">
            {msg.attachments.map(att => (
              <AttachmentImage
                key={att.hash}
                circleId={circleId}
                att={att}
                nonce={blobNonces[att.hash] ?? 0}
                onOpen={() => onOpenImage(att.hash)}
              />
            ))}
          </div>
        )}
      </div>
    </article>
  )
}

/** A message composed but not yet sent, held for one circle. */
interface Draft {
  nodes: DraftNode[]
  attachments: Attachment[]
}

const EMPTY_DRAFT: Draft = { nodes: [], attachments: [] }

export default function ChatPanel({ activityContainer, onActivityNavigate, onMessage, variant = 'rail', hideActiveCircleGlyph = false }: Props) {
  const { activeCircleId, circles, status } = useApp()
  const activeCircle = circles.find(c => c.circle_id === activeCircleId)
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [chatLoaded, setChatLoaded] = useState(false)
  const [chatError, setChatError] = useState<string | null>(null)
  const [members, setMembers] = useState<Member[]>([])
  const [presence, setPresence] = useState<Presence[]>([])
  const [activities, setActivities] = useState<Record<string, ChatActivity>>({})
  const [activityClock, setActivityClock] = useState(() => Math.floor(Date.now() / 1000))
  // What the next message will do if it names no agent. Implicit routing that
  // is invisible is a bug, so the composer says so before you press Enter.
  const [engagement, setEngagement] = useState<EngagementView | null>(null)
  // Plaintext value of the input, mirrored from MentionInput for send.
  const [input, setInput] = useState('')
  // The active `@fragment` under the caret (drives the popup), or null.
  const [fragment, setFragment] = useState<string | null>(null)
  const [mentionIndex, setMentionIndex] = useState(0)
  // True once the user has navigated the popup with arrows — only then does
  // Enter accept a suggestion instead of sending the message.
  const [mentionActive, setMentionActive] = useState(false)
  // Attachments uploaded and awaiting send with the next message.
  const [pending, setPending] = useState<Attachment[]>([])
  // filename -> 0..1. An entry exists only while that file is uploading.
  const [uploads, setUploads] = useState<Record<string, number>>({})
  const [attachError, setAttachError] = useState<string | null>(null)
  // Per-hash counter bumped when the daemon finishes fetching a blob, so an
  // image that 404'd on first render retries.
  const [blobNonces, setBlobNonces] = useState<Record<string, number>>({})
  const [dragging, setDragging] = useState(false)
  // Hash of the image open in the viewer, or null when it is closed.
  const [lightboxHash, setLightboxHash] = useState<string | null>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const inputRef = useRef<MentionInputHandle>(null)
  const messageListRef = useRef<HTMLDivElement>(null)
  const bottomRef = useRef<HTMLDivElement>(null)
  const hydratingChatRef = useRef(false)
  const seenRef = useRef(new Set<string>())
  const latestTsRef = useRef<number | null>(null)
  const typingLastSentRef = useRef(0)
  const typingClearRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // Last plaintext the composer reported, so a report that merely repeats the
  // current contents can be told apart from someone actually typing.
  const lastTypedTextRef = useRef('')
  // Unsent composer contents, one entry per circle. The panel stays mounted
  // across a circle switch, so without this a half-written message and any
  // staged image would follow you into the next circle — and send there.
  const draftsRef = useRef<Record<string, Draft>>({})
  // The circle the composer's current contents belong to. Only the switch
  // effect below moves it, so every write attributes to the right circle even
  // when it lands in the same commit as a switch.
  const draftCircleRef = useRef<string | null>(activeCircleId)

  const writeDraft = useCallback((patch: Partial<Draft>) => {
    const id = draftCircleRef.current
    if (!id) return
    draftsRef.current[id] = { ...(draftsRef.current[id] ?? EMPTY_DRAFT), ...patch }
  }, [])

  // Staged attachments change through several paths (upload, send, failed
  // send), so mirror them into the draft from one place rather than each.
  useEffect(() => { writeDraft({ attachments: pending }) }, [pending, writeDraft])

  // Park the outgoing circle's draft and bring back the incoming one. The
  // outgoing draft is already stored — every edit above writes it as it
  // happens — so this only has to swap what the composer is showing.
  useEffect(() => {
    if (draftCircleRef.current === activeCircleId) return
    draftCircleRef.current = activeCircleId
    const draft = (activeCircleId && draftsRef.current[activeCircleId]) || EMPTY_DRAFT
    const text = draftText(draft.nodes)
    inputRef.current?.restore(draft.nodes)
    setInput(text)
    setPending(draft.attachments)
    setFragment(null)
    setMentionActive(false)
    setMentionIndex(0)
    setAttachError(null)
    // A restore is not a keystroke: keep the typing indicator out of it.
    lastTypedTextRef.current = text
  }, [activeCircleId])

  const ingestActivity = useCallback((activity: ChatActivity) => {
    setActivities(prev => {
      if (activity.expires_at <= Math.floor(Date.now() / 1000)) {
        if (!(activity.activity_id in prev)) return prev
        const next = { ...prev }
        delete next[activity.activity_id]
        return next
      }
      return { ...prev, [activity.activity_id]: activity }
    })
  }, [])

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
    hydratingChatRef.current = true
    seenRef.current.clear()
    latestTsRef.current = null
    setMessages([])
    setChatLoaded(false)
    setChatError(null)
    setMembers([])
    setPresence([])
    setActivities({})

    const refreshRoster = () => {
      getMembers(activeCircleId).then(m => { if (!cancelled) setMembers(m) }).catch(() => {})
      getWho(activeCircleId).then(p => { if (!cancelled) setPresence(p) }).catch(() => {})
    }
    refreshRoster()
    getChatActivity(activeCircleId)
      .then(items => { if (!cancelled) items.forEach(ingestActivity) })
      .catch(() => {})

    // This is the only backfill path — there is no chat poll, and the SSE
    // stream stays healthy — so a swallowed failure here used to leave an empty
    // transcript marked as loaded, with nothing to correct it short of a page
    // reload. The daemon answers 503 circle_busy whenever the control doc is
    // locked, which is common right after load. Retry, and never claim the
    // transcript is loaded on a request that failed.
    let catchUpTimer: number | undefined
    let catchUpAttempts = 0

    const finishCatchUp = () => {
      if (cancelled) return
      window.requestAnimationFrame(() => {
        if (cancelled) return
        const list = messageListRef.current
        if (list) list.scrollTo({ top: list.scrollHeight, behavior: 'auto' })
        hydratingChatRef.current = false
        setChatLoaded(true)
      })
    }

    const catchUp = () => {
      const since = latestTsRef.current === null ? undefined : Math.max(0, latestTsRef.current - 1)
      getChat(activeCircleId, since)
        .then(msgs => {
          if (cancelled) return
          catchUpAttempts = 0
          setChatError(null)
          msgs.forEach(addMsg)
          finishCatchUp()
        })
        .catch((err: unknown) => {
          if (cancelled) return
          catchUpAttempts += 1
          if (catchUpAttempts >= CATCH_UP_MAX_ATTEMPTS) {
            const msg = err instanceof Error ? err.message : String(err)
            setChatError(msg)
            return
          }
          catchUpTimer = window.setTimeout(catchUp, CATCH_UP_BACKOFF_MS * catchUpAttempts)
        })
    }

    const es = chatStream(activeCircleId)
    es.onopen = catchUp
    es.addEventListener('message', e => {
      try {
        const data = JSON.parse(e.data)
        if (data.type === 'message_posted') addMsg(data.message)
        if (data.type === 'chat_activity_changed') ingestActivity(data.activity)
        if (data.type === 'attachment_available' && data.hash) {
          setBlobNonces(prev => ({ ...prev, [data.hash]: (prev[data.hash] ?? 0) + 1 }))
        }
        if (data.type === 'member_added' || data.type === 'member_removed' || data.type === 'presence_changed') {
          refreshRoster()
        }
      } catch {}
    })
    catchUp()
    return () => {
      cancelled = true
      if (catchUpTimer !== undefined) window.clearTimeout(catchUpTimer)
      es.close()
    }
  }, [activeCircleId, addMsg, ingestActivity])

  useEffect(() => {
    const timer = window.setInterval(
      () => setActivityClock(Math.floor(Date.now() / 1000)),
      1000,
    )
    return () => window.clearInterval(timer)
  }, [])

  const stopTyping = useCallback(() => {
    if (typingClearRef.current) {
      clearTimeout(typingClearRef.current)
      typingClearRef.current = null
    }
    const wasTyping = typingLastSentRef.current > 0
    typingLastSentRef.current = 0
    if (wasTyping && activeCircleId && status?.agent_id) {
      setChatTyping(activeCircleId, status.agent_id, false).catch(() => {})
    }
  }, [activeCircleId, status?.agent_id])

  useEffect(() => () => stopTyping(), [stopTyping])

  useEffect(() => {
    if (hydratingChatRef.current) return
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  // My owner name — used to recognise all my devices as "you"
  const myMember = members.find(m => m.agent_id === status?.agent_id)
  const selfOwner = myMember
    ? peerLabel(myMember.owner, myMember.agent_id)
    : shortenAgentId(status?.agent_id ?? '')

  const getSenderLabel = useCallback((agentId: string, peerId?: string): SenderLabel => {
    // Direct member lookup by device agent_id
    let member = members.find(m => m.agent_id === agentId)
    let agentName: string | null = null

    // Not a device's own id, so it is an agent name (e.g. "codex"). Prefer the
    // posting peer: several devices may configure the same agent, and matching
    // on the name alone picks whichever member happens to be listed first —
    // attributing a run to the wrong device. Fall back to the name only for
    // messages from peers that predate the peer_id field.
    if (!member) {
      const host = (peerId && members.find(m => m.peer_id === peerId))
        || members.find(m => m.agents.includes(agentId))
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

  /** Upload dropped/pasted/picked files and stage them for the next send.
   *  Each upload is independent: one rejected file does not discard the rest. */
  const addFiles = useCallback(async (files: File[]) => {
    if (!activeCircleId || activeCircle?.disabled) return
    const images = files.filter(f => f.type.startsWith('image/'))
    if (!images.length) {
      if (files.length) setAttachError('Only images can be attached.')
      return
    }
    setAttachError(null)
    for (const file of images) {
      // Check client-side too, so an oversized paste fails instantly instead of
      // after uploading megabytes the daemon will reject.
      if (file.size > MAX_ATTACHMENT_BYTES) {
        setAttachError(`${file.name} is too large (max ${formatBytes(MAX_ATTACHMENT_BYTES)}).`)
        continue
      }
      const key = `${file.name}:${file.size}:${file.lastModified}`
      setUploads(prev => ({ ...prev, [key]: 0 }))
      try {
        const att = await uploadAttachment(activeCircleId, file, fraction => {
          setUploads(prev => (key in prev ? { ...prev, [key]: fraction } : prev))
        })
        // Same bytes twice is the same blob — don't stage a duplicate.
        setPending(prev => prev.some(p => p.hash === att.hash) ? prev : [...prev, att])
      } catch (error) {
        setAttachError(error instanceof Error ? error.message : 'Upload failed.')
      } finally {
        setUploads(prev => {
          const next = { ...prev }
          delete next[key]
          return next
        })
      }
    }
  }, [activeCircleId, activeCircle?.disabled])

  const onPaste = useCallback((e: React.ClipboardEvent) => {
    const files = Array.from(e.clipboardData?.files ?? [])
    if (files.length) {
      // Don't also paste the image's filename as text.
      e.preventDefault()
      void addFiles(files)
    }
  }, [addFiles])

  const onDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault()
    setDragging(false)
    void addFiles(Array.from(e.dataTransfer?.files ?? []))
  }, [addFiles])

  const uploadEntries = Object.entries(uploads)

  // Every image in the transcript, in order, so the viewer can page through
  // the whole conversation rather than just the message that was clicked.
  const galleryItems = messages.flatMap(m => m.attachments ?? [])
  const lightboxIndex = lightboxHash
    ? galleryItems.findIndex(a => a.hash === lightboxHash)
    : -1

  const send = () => {
    const text = input.trim()
    if (!activeCircleId || !status || activeCircle?.disabled) return
    // An image on its own is a valid message; empty text with nothing staged
    // is not. Block send while an upload is still in flight so the attachment
    // isn't silently dropped from the message.
    if ((!text && pending.length === 0) || Object.keys(uploads).length > 0) return

    // Capture what the composer holds before clearing it. The clear is
    // optimistic — it has to be, or sending feels laggy — so the only way a
    // failure does not cost the user their message is to keep a copy.
    const sentNodes = (activeCircleId && draftsRef.current[activeCircleId]?.nodes) || []
    const sentCircleId = activeCircleId

    inputRef.current?.clear()
    setInput('')
    setFragment(null)
    setMentionActive(false)
    stopTyping()
    const attachments = pending.map(a => ({ hash: a.hash, name: a.name }))
    const sentAttachments = pending
    setPending([])
    writeDraft({ nodes: [] })
    setAttachError(null)
    const sentReplyTo = replyTo
    setReplyTo(null)
    postChat(activeCircleId, text, status.agent_id, attachments, sentReplyTo?.id).catch(error => {
      // Put the message back exactly as it was — text included. Losing a long
      // message to a moment of contention is worse than any error copy.
      //
      // Only restore into the composer if the user is still looking at the
      // circle they sent from; otherwise park it in that circle's draft, where
      // switching back will bring it up.
      const restored = { nodes: sentNodes, attachments: sentAttachments }
      if (draftCircleRef.current === sentCircleId) {
        inputRef.current?.restore(sentNodes)
        setInput(text)
        setPending(prev => [...sentAttachments, ...prev])
        setReplyTo(sentReplyTo)
        lastTypedTextRef.current = text
        setAttachError(sendErrorText(error))
      } else if (sentCircleId) {
        draftsRef.current[sentCircleId] = restored
      }
    })
  }

  const mentionOpen = fragment !== null

  // MentionInput reports the plaintext value and the active @fragment together.
  const onInputChange = (text: string, frag: string | null, nodes: DraftNode[]) => {
    setInput(text)
    setFragment(frag)
    writeDraft({ nodes })
    // Typing changes the filter — reset navigation so Enter sends until the
    // user explicitly arrows into the list again.
    setMentionActive(false)
    setMentionIndex(0)

    // The composer reports its value on every render, not only on a keystroke,
    // and a circle switch replays the restored draft through the same path.
    // Only a real change to the text is someone typing — otherwise opening a
    // circle that has a saved draft would announce you as typing in it.
    const edited = text !== lastTypedTextRef.current
    lastTypedTextRef.current = text
    if (!edited) return

    if (!activeCircleId || !status?.agent_id || activeCircle?.disabled) return
    if (typingClearRef.current) clearTimeout(typingClearRef.current)
    if (!text.trim()) {
      stopTyping()
      return
    }
    const now = Date.now()
    if (now - typingLastSentRef.current >= 2000) {
      typingLastSentRef.current = now
      setChatTyping(activeCircleId, status.agent_id, true).catch(() => {})
    }
    typingClearRef.current = setTimeout(stopTyping, 4000)
  }

  const applyMention = (item: MentionItem) => {
    inputRef.current?.insertMention(item.insert)
    setFragment(null)
    setMentionActive(false)
  }

  const onInputKeyDown = (e: React.KeyboardEvent) => {
    // Ignore every key that belongs to an in-progress IME composition.
    //
    // Typing Chinese, Japanese or Korean goes through a candidate window, and
    // Enter there means "accept this candidate", not "send". Arrow keys move
    // through candidates for the same reason. Acting on those posts a half
    // finished message mid-word — so anyone using Pinyin or a Japanese IME
    // cannot write a sentence without it being sent out from under them.
    //
    // `isComposing` is the standardised signal and covers the whole
    // composition; keyCode 229 is the legacy equivalent some browsers still
    // send for the final key, so check both.
    const native = e.nativeEvent as KeyboardEvent
    if (native.isComposing || native.keyCode === 229) return

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
    if (e.key === 'Escape' && replyTo) {
      e.preventDefault()
      setReplyTo(null)
      return
    }
    if (e.key === 'Escape' && engagement?.agent) {
      // Esc with no popup open leaves the conversation: the next message needs
      // a mention again.
      e.preventDefault()
      dismissEngagement()
      return
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      send()
    }
  }

  const hasConversation = messages.length > 0
  const compactDock = hasConversation || !chatLoaded
  const onlineCount = presence.filter(p => p.status === 'online').length
  const glyphSize = compactDock ? 64 : 88
  const liveActivities = Object.values(activities)
    .filter(activity => activity.expires_at > activityClock && activity.actor_id !== status?.agent_id)
    .sort((a, b) => a.updated_at - b.updated_at)

  // A delegation cascade currently running: an agent is working on a message
  // that an agent (not a person) asked for. This is the only case that needs a
  // brake — a turn a person typed is one they are waiting for.
  //
  // Stopping does not interrupt the turn in flight; it stops every further one,
  // on every device, which is what actually bounds the spend.
  const runningCascadeRoot = (() => {
    for (const activity of liveActivities) {
      if (activity.kind !== 'working' || !activity.message_id) continue
      const source = messages.find(m => m.id === activity.message_id)
      const relay = source?.relay
      if (relay?.root && (relay.path?.length ?? 0) > 0) return relay.root
    }
    return null
  })()

  // An explicit reply target, set by the "reply" affordance on an agent
  // message. Outranks the follow-up window: pointing beats guessing.
  const [replyTo, setReplyTo] = useState<{ id: string; agent: string } | null>(null)
  const startReply = useCallback((msg: ChatMessage) => {
    const path = msg.relay?.path ?? []
    const agent = path.length > 0 ? path[path.length - 1] : msg.agent_id
    const member = members.find(m => m.peer_id === msg.peer_id)
    const recipient = member?.owner && member?.device_label ? `${member.owner}/${member.device_label}/${agent}` : `${agent}${msg.peer_id ? ` on ${msg.peer_id.slice(-8)}` : ''}`
    setReplyTo({ id: msg.id, agent: recipient })
    inputRef.current?.focus()
  }, [members])

  const refreshEngagement = useCallback(() => {
    if (!activeCircleId) return
    getEngagement(activeCircleId).then(setEngagement).catch(() => {})
  }, [activeCircleId])

  // Re-read whenever the transcript changes: a reply opens the window, and
  // sending into it moves it on.
  useEffect(() => {
    refreshEngagement()
  }, [refreshEngagement, messages.length])

  const dismissEngagement = useCallback(async () => {
    if (!activeCircleId) return
    // Clear locally first so Esc feels instant; the daemon is the record.
    setEngagement(prev => (prev ? { ...prev, agent: null } : prev))
    try {
      await exitEngagement(activeCircleId)
    } finally {
      refreshEngagement()
    }
  }, [activeCircleId, refreshEngagement])

  const [stoppingRoot, setStoppingRoot] = useState<string | null>(null)
  const haltCascade = useCallback(async (root: string) => {
    if (!activeCircleId) return
    setStoppingRoot(root)
    try {
      await stopRelay(activeCircleId, root)
    } catch {
      // A failed stop must not look like a successful one — clearing the
      // pending state puts the button back so the user can try again.
      setStoppingRoot(null)
    }
  }, [activeCircleId])

  const describeActivity = (activity: ChatActivity) => {
    const label = getSenderLabel(activity.actor_id, activity.peer_id)
    const host = activity.peer_id ? members.find(member => member.peer_id === activity.peer_id) : undefined
    const actor = label.agent
      ? `${label.agent}${host?.device_label ? ` · ${host.device_label}` : ''}`
      : `${label.user}${label.device ? ` · ${label.device}` : ''}`
    if (activity.kind === 'typing') return `${actor} is typing…`
    if (activity.kind === 'seen') return `${actor} saw the message`
    // Considered and not run. Saying so is the whole point: silence that looks
    // identical to a crashed adapter is what stops people trusting the Circle.
    if (activity.kind === 'skipped') {
      return `${actor} not triggered${activity.detail ? ` · ${activity.detail}` : ''}`
    }
    return `${actor} is working…`
  }

  return (
    <main className={`app-chat-panel flex min-h-0 flex-col z-10 overflow-hidden ${variant === 'main' ? 'chat-main sys-window' : 'border-r-2 border-obsidian bg-alabaster/85'}`}>
      {variant !== 'main' && (
        <div className="section-header">
          <span>Terminal Log</span>
        </div>
      )}

      {variant === 'main' && activeCircle && (
        <div
          className={`active-circle-dock${compactDock ? ' active-circle-dock--compact' : ' active-circle-dock--empty'}${activeCircle.disabled ? ' active-circle-dock--void' : ''}${hideActiveCircleGlyph ? ' active-circle-dock--ritual' : ''}`}
        >
          <div className="active-circle-dock__meta">
            <span>{activeCircle.circle_name}</span>
            <strong>{activeCircle.disabled ? 'DISABLED' : 'ACTIVE CIRCLE'}</strong>
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
          <span>DISABLED</span>
          <strong>THIS CIRCLE IS DISABLED · ENABLE IT TO RESUME</strong>
        </div>
      )}

      <div ref={messageListRef} className="chat-message-list">
        {messages.length === 0 && chatLoaded && !chatError && (
          <div className="chat-empty-state">
            <span>NO MESSAGES YET</span>
            <strong>Send the first message.</strong>
          </div>
        )}
        {chatError && (
          <div className="chat-empty-state">
            <span>COULD NOT LOAD MESSAGES</span>
            <strong>{chatError}</strong>
          </div>
        )}
        {messages.map((msg, i) => {
          const label = getSenderLabel(msg.agent_id, msg.peer_id)
          const isThisDevice = msg.agent_id === status?.agent_id
          const isMine = label.user === 'you'
          const prev = messages[i - 1]
          const startsNewDay = !prev || calendarDay(prev.ts) !== calendarDay(msg.ts)
          // Same agent name from two devices is two speakers, so the peer is
          // part of the grouping key.
          const showSender = !prev
            || prev.agent_id !== msg.agent_id
            || prev.peer_id !== msg.peer_id
            || msg.ts - prev.ts > 300
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
                circleId={activeCircleId ?? ''}
                blobNonces={blobNonces}
                onOpenImage={setLightboxHash}
                onReply={startReply}
                replyParent={messages.find(parent => parent.id === msg.reply_to)}
              />
            </Fragment>
          )
        })}
        <div ref={bottomRef} />
      </div>

      {activeCircleId && activityContainer && createPortal(<ExecutionStatus onNavigate={onActivityNavigate} circleId={activeCircleId} members={members} messages={messages} />, activityContainer)}
      <div
        className={`chat-composer${dragging ? ' chat-composer--dragging' : ''}`}
        onDragOver={e => { e.preventDefault(); if (!activeCircle?.disabled) setDragging(true) }}
        onDragLeave={e => {
          // Ignore bubbling leaves from children, which would flicker the state.
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false)
        }}
        onDrop={onDrop}
      >
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
        {replyTo && (
          <div className="chat-engagement" role="status" aria-live="polite">
            <span>
              replying to <strong>@{replyTo.agent}</strong> · this message only
            </span>
            <button type="button" onClick={() => setReplyTo(null)} title="Cancel this reply">
              cancel
            </button>
          </div>
        )}
        {!replyTo && engagement?.agent && (
          <div className="chat-engagement" role="status" aria-live="polite">
            <span>
              replying to <strong>@{engagement.agent}</strong>
              {liveActivities.some(a => a.actor_id === engagement.agent && a.kind === 'working')
                ? ' · working, your message will be queued'
                : ' · no mention needed'}
            </span>
            <button type="button" onClick={dismissEngagement} title="Stop replying to this agent (Esc)">
              esc to exit
            </button>
          </div>
        )}
        {liveActivities.length > 0 && (
          <div className="chat-activity" role="status" aria-live="polite">
            {liveActivities.slice(0, 3).map(activity => (
              <span key={activity.activity_id} className={`chat-activity__item chat-activity__item--${activity.kind}`}>
                <i aria-hidden="true" />
                {describeActivity(activity)}
              </span>
            ))}
            {liveActivities.length > 3 && <span>+{liveActivities.length - 3} active</span>}
            {runningCascadeRoot && (
              <button
                type="button"
                className="chat-activity__stop"
                onClick={() => haltCascade(runningCascadeRoot)}
                disabled={stoppingRoot === runningCascadeRoot}
                title="Agents are handing work to each other. Stop the rest of this chain."
              >
                {stoppingRoot === runningCascadeRoot ? 'stopping…' : 'stop chain'}
              </button>
            )}
          </div>
        )}
        {(pending.length > 0 || uploadEntries.length > 0 || attachError) && (
          <div className="chat-composer__attachments">
            {pending.map(att => (
              <div key={att.hash} className="chat-staged">
                <img src={blobUrl(activeCircleId ?? '', att.hash)} alt={att.name} />
                <button
                  type="button"
                  className="chat-staged__remove"
                  aria-label={`Remove ${att.name}`}
                  onClick={() => setPending(prev => prev.filter(p => p.hash !== att.hash))}
                >
                  ×
                </button>
              </div>
            ))}
            {uploadEntries.map(([key, fraction]) => {
              const pct = Math.round(fraction * 100)
              return (
                <span
                  key={key}
                  className="chat-staged chat-staged--busy"
                  role="progressbar"
                  aria-valuenow={pct}
                  aria-valuemin={0}
                  aria-valuemax={100}
                  aria-label={`Uploading ${key.split(':')[0]}`}
                >
                  <span className="chat-staged__fill" style={{ width: `${pct}%` }} />
                  <span className="chat-staged__pct">{pct}%</span>
                </span>
              )
            })}
            {attachError && (
              <span className="chat-composer__attach-error" role="alert">{attachError}</span>
            )}
          </div>
        )}
        <input
          ref={fileInputRef}
          type="file"
          accept="image/png,image/jpeg,image/gif,image/webp"
          multiple
          hidden
          onChange={e => {
            void addFiles(Array.from(e.target.files ?? []))
            // Reset so picking the same file twice still fires a change event.
            e.target.value = ''
          }}
        />
        <button
          type="button"
          className="enox-btn chat-composer__attach"
          onClick={() => fileInputRef.current?.click()}
          disabled={activeCircle?.disabled}
          title="Attach an image"
          aria-label="Attach an image"
        >
          +
        </button>
        <MentionInput
          ref={inputRef}
          onChange={onInputChange}
          onKeyDown={onInputKeyDown}
          placeholder={activeCircle?.disabled ? 'Circle disabled — enable to resume' : replyTo?.agent || engagement?.agent ? `Reply to @${replyTo?.agent || engagement?.agent}…` : 'Message the circle...  (@ to mention)'}
          className={`chat-composer__input${activeCircle?.disabled ? ' chat-composer__input--disabled' : ''}`}
          disabled={activeCircle?.disabled}
          onPaste={onPaste}
        />
        <button onClick={send} disabled={activeCircle?.disabled || Object.keys(uploads).length > 0} className="enox-btn chat-composer__send">
          {activeCircle?.disabled ? 'VOID' : variant === 'main' ? 'SEND' : 'EXEC'}
        </button>
      </div>

      {lightboxIndex >= 0 && (
        <Lightbox
          circleId={activeCircleId ?? ''}
          items={galleryItems}
          index={lightboxIndex}
          onIndexChange={i => setLightboxHash(galleryItems[i]?.hash ?? null)}
          onClose={() => setLightboxHash(null)}
        />
      )}
    </main>
  )
}
