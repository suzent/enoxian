import { useCallback, useEffect, useState } from 'react'
import { createAgentCard, getAgentCards, type AgentCard } from '../api'

interface Props {
  circleId: string
  /** Open a shared-folder file in the editor. Absent, paths are shown as text. */
  onOpenFile?: (path: string) => void
}

/**
 * Agent cards in this Circle: one Markdown file per agent per device under
 * `agents/`, which agents read to know who is good for what, and which the
 * warden reads as the description of each option. Only this device's agents
 * can get a card from here; anyone's card can be opened and edited like any
 * other shared file, and every edit is a proposal.
 */
export default function AgentCards({ circleId, onOpenFile }: Props) {
  const [cards, setCards] = useState<AgentCard[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)

  const refresh = useCallback(() => {
    getAgentCards(circleId).then(r => setCards(r.cards)).catch(e => setError(e.message))
  }, [circleId])
  useEffect(() => { refresh() }, [refresh])

  const create = async (card: AgentCard) => {
    setBusy(card.agent)
    setError(null)
    try {
      const { path } = await createAgentCard(circleId, card.agent)
      refresh()
      onOpenFile?.(path)
    } catch (e: any) {
      setError(e.message)
    } finally {
      setBusy(null)
    }
  }

  if (!cards) return error ? <p role="alert" className="text-[11px] font-mono">{error}</p> : null
  const local = cards.filter(c => c.local)
  const others = cards.filter(c => !c.local)

  const row = (card: AgentCard) => (
    <div key={card.handle} className="py-2 font-mono flex items-start justify-between gap-3">
      <div className="min-w-0">
        <div className="text-[11px] font-bold truncate">{card.handle}</div>
        <div className={`text-[10px] mt-0.5 ${card.summary ? 'text-obsidian' : 'text-slate'}`}>
          {card.exists ? (card.summary || 'Card has no summary yet') : 'No card'}
        </div>
      </div>
      {card.exists ? (
        onOpenFile && (
          <button className="enox-btn text-[9px] px-2 py-1 min-h-0" onClick={() => onOpenFile(card.path)}
            title={card.path}>OPEN</button>
        )
      ) : card.local ? (
        <button className="enox-btn text-[9px] px-2 py-1 min-h-0 disabled:opacity-50" disabled={busy !== null}
          onClick={() => void create(card)} title={`Write a template at ${card.path}`}>
          {busy === card.agent ? 'WRITING…' : 'CREATE CARD'}
        </button>
      ) : null}
    </div>
  )

  return (
    <section className="mt-4">
      <div className="flex items-baseline justify-between border-b border-obsidian pb-1 mb-1">
        <span className="font-mono text-[11px] font-bold">AGENT CARDS</span>
        <span className="font-mono text-[8px] text-slate">SHARED · agents/</span>
      </div>
      <p className="text-[10px] text-slate font-mono py-1">
        What each agent says it is good for. Agents read these to know who to hand work to, and
        the warden reads them when choosing who answers. Edits are reviewed like any other change.
      </p>
      {error && <p role="alert" className="text-[11px] font-mono">{error}</p>}
      {cards.length === 0 && <p className="text-[10px] text-slate font-mono py-1">No agents are advertised in this Circle yet.</p>}
      <div className="divide-y divide-obsidian/20">
        {local.map(row)}
        {others.map(row)}
      </div>
    </section>
  )
}
