import { useState, useEffect, useRef, useCallback } from 'react'
import type { ChatMessage } from '../types'
import { getChat, postChat, chatStream } from '../api'
import { useApp } from '../context/AppContext'

interface Props {
  onMessage?: () => void
}

function formatTime(ts: number) {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

function MsgRow({ msg, myAgentId }: { msg: ChatMessage; myAgentId: string }) {
  const isMe = msg.agent_id === myAgentId
  const isSystem = msg.agent_id === 'system'

  if (isSystem) {
    return (
      <div className="msg-system px-2 py-1 text-alabaster bg-obsidian font-mono text-[11px] w-fit
                      bg-scanline">
        {msg.text}
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-1">
      <div className="flex justify-between font-mono text-[11px] font-bold">
        <span>@{msg.agent_id}{isMe ? ' (ME)' : ''}</span>
        <span className="font-normal text-slate">{formatTime(msg.ts)}</span>
      </div>
      <div
        className="pl-2 font-mono text-[11px] leading-relaxed break-all overflow-hidden"
        style={{
          borderLeft: isMe ? '4px solid #111111' : '2px dashed #111111',
          wordBreak: 'break-word',
          overflowWrap: 'anywhere',
        }}
      >
        {msg.text}
      </div>
    </div>
  )
}

export default function ChatPanel({ onMessage }: Props) {
  const { activeCircleId, status } = useApp()
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [input, setInput] = useState('')
  const bottomRef = useRef<HTMLDivElement>(null)
  const seenRef = useRef(new Set<string>())

  const addMsg = useCallback((msg: ChatMessage) => {
    if (seenRef.current.has(msg.id)) return
    seenRef.current.add(msg.id)
    setMessages(prev => [...prev, msg])
    onMessage?.()
  }, [onMessage])

  useEffect(() => {
    if (!activeCircleId) return
    seenRef.current.clear()
    setMessages([])

    getChat(activeCircleId).then(msgs => msgs.forEach(addMsg)).catch(() => {})

    const es = chatStream(activeCircleId)
    es.addEventListener('message', e => {
      try {
        const data = JSON.parse(e.data)
        if (data.type === 'message_posted') addMsg(data.message)
      } catch {}
    })
    return () => es.close()
  }, [activeCircleId, addMsg])

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  const send = () => {
    const text = input.trim()
    if (!text || !activeCircleId || !status) return
    setInput('')
    postChat(activeCircleId, text, status.agent_id).catch(() => {})
  }

  return (
    <aside className="flex flex-col border-r-2 border-obsidian bg-alabaster/85 z-10 overflow-hidden">
      <div className="section-header">Terminal Log</div>

      <div className="flex-1 overflow-y-auto overflow-x-hidden px-4 py-4 flex flex-col gap-4 font-mono text-[11px] min-w-0">
        {messages.length === 0 && (
          <div className="text-slate">[SYS] AWAITING TRANSMISSION...</div>
        )}
        {messages.map(msg => (
          <MsgRow key={msg.id} msg={msg} myAgentId={status?.agent_id ?? ''} />
        ))}
        <div ref={bottomRef} />
      </div>

      <div className="border-t-2 border-obsidian p-4 flex gap-2 bg-alabaster">
        <input
          value={input}
          onChange={e => setInput(e.target.value)}
          onKeyDown={e => e.key === 'Enter' && send()}
          placeholder="Inject command..."
          className="flex-1 bg-transparent border border-obsidian font-mono text-[11px] px-2 py-2
                     text-obsidian placeholder:text-slate focus:outline-none focus:bg-obsidian/5"
        />
        <button onClick={send} className="enoch-btn">EXEC</button>
      </div>
    </aside>
  )
}
