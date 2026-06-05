import { useState, useEffect, useRef, useCallback } from 'react'
import type { ChatMessage, Member } from '../types'
import { getChat, postChat, chatStream, getMembers } from '../api'
import { useApp } from '../context/AppContext'
import { shortenAgentId, peerLabel } from '../lib/displayName'
import CircleGlyph from './CircleGlyph'

interface Props {
  onMessage?: () => void
  variant?: 'rail' | 'main'
}

function formatTime(ts: number) {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

interface SenderLabel {
  user: string    // "you" or owner name
  device: string | null  // shown when owner has multiple devices
  agent: string | null   // shown when a registered agent (not device primary) is speaking
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
      <div className="self-center px-3 py-1 text-alabaster bg-obsidian font-mono text-[10px] bg-scanline">
        {msg.text}
      </div>
    )
  }

  return (
    <div className={`flex flex-col gap-0.5 max-w-[78%] ${isThisDevice ? 'self-end items-end' : 'self-start items-start'}`}>
      {showSender && (
        <div className={`flex items-baseline gap-1 px-0.5 ${isThisDevice ? 'flex-row-reverse' : ''}`}>
          <span className="text-[9px] font-bold font-mono text-slate">{label.user}</span>
          {label.device && <span className="text-[9px] font-mono text-slate/50">· {label.device}</span>}
          {label.agent && <span className="text-[9px] font-mono text-slate/50">· {label.agent}</span>}
        </div>
      )}
      <div
        className={`px-3 py-2 font-mono text-[11px] leading-relaxed ${
          isMine
            ? 'bg-obsidian text-alabaster'
            : 'bg-alabaster border-2 border-obsidian text-obsidian'
        }`}
        style={{ wordBreak: 'break-word', overflowWrap: 'anywhere' }}
      >
        {msg.text}
      </div>
      <span className="text-[9px] text-slate/50 font-mono px-0.5">{formatTime(msg.ts)}</span>
    </div>
  )
}

export default function ChatPanel({ onMessage, variant = 'rail' }: Props) {
  const { activeCircleId, circles, status } = useApp()
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [members, setMembers] = useState<Member[]>([])
  const [input, setInput] = useState('')
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

    getMembers(activeCircleId).then(m => { if (!cancelled) setMembers(m) }).catch(() => {})

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
        if (data.type === 'member_joined' || data.type === 'member_removed') {
          getMembers(activeCircleId).then(m => { if (!cancelled) setMembers(m) }).catch(() => {})
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
    if (!text || !activeCircleId || !status) return
    setInput('')
    postChat(activeCircleId, text, status.agent_id).catch(() => {})
  }

  const activeCircle = circles.find(c => c.circle_id === activeCircleId)

  return (
    <main className={`app-chat-panel flex min-h-0 flex-col z-10 overflow-hidden ${variant === 'main' ? 'chat-main sys-window' : 'border-r-2 border-obsidian bg-alabaster/85'}`}>
      {variant !== 'main' && (
        <div className="section-header">
          <span>Terminal Log</span>
        </div>
      )}

      {variant === 'main' && activeCircle && (
        <div className={`active-circle-dock${activeCircle.disabled ? ' active-circle-dock--void' : ''}`} data-circle-dock>
          <div className="active-circle-dock__meta">
            <span>{activeCircle.circle_name}</span>
            {activeCircle.disabled && <strong>VOID</strong>}
          </div>
          <CircleGlyph
            name={activeCircle.circle_name}
            size={88}
            className="active-circle-dock__glyph"
            title={activeCircle.circle_name}
            voided={activeCircle.disabled}
          />
        </div>
      )}

      <div className="flex-1 overflow-y-auto overflow-x-hidden px-4 py-4 flex flex-col gap-2 font-mono text-[11px] min-w-0">
        {messages.length === 0 && (
          <div className="text-slate self-start">[SYS] AWAITING TRANSMISSION...</div>
        )}
        {messages.map((msg, i) => {
          const label = getSenderLabel(msg.agent_id)
          const isThisDevice = msg.agent_id === status?.agent_id
          const isMine = label.user === 'you'
          const prev = messages[i - 1]
          const showSender = !prev || prev.agent_id !== msg.agent_id
          return (
            <Bubble
              key={msg.id}
              msg={msg}
              isMine={isMine}
              isThisDevice={isThisDevice}
              label={label}
              showSender={showSender}
            />
          )
        })}
        <div ref={bottomRef} />
      </div>

      <div className="border-t-2 border-obsidian p-4 flex flex-wrap gap-2 bg-alabaster">
        <input
          value={input}
          onChange={e => setInput(e.target.value)}
          onKeyDown={e => e.key === 'Enter' && send()}
          placeholder="Inject command..."
          className="min-w-[160px] flex-1 bg-transparent border border-obsidian font-mono text-[11px] px-2 py-2
                     text-obsidian placeholder:text-slate focus:outline-none focus:bg-obsidian/5"
        />
        <button onClick={send} className="enox-btn">{variant === 'main' ? 'SEND' : 'EXEC'}</button>
      </div>
    </main>
  )
}
