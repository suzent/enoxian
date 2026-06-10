import { useState, useCallback } from 'react'
import type { Proposal, ProposalDetail } from '../types'
import { getProposalDetail, acceptProposal, rejectProposal, revertProposal } from '../api'

interface Props {
  circleId: string
  proposals: Proposal[]
  onChanged: () => void
}

function age(isoStr: string) {
  const secs = Math.floor((Date.now() - new Date(isoStr).getTime()) / 1000)
  if (secs < 60) return 'just now'
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`
  return `${Math.floor(secs / 3600)}h ago`
}

const STATUS_STYLE: Record<Proposal['status'], string> = {
  pending: 'border-obsidian bg-obsidian text-alabaster',
  accepted: 'border-obsidian',
  synced: 'border-obsidian',
  conflicted: 'border-obsidian text-obsidian',
  rejected: 'border-slate text-slate',
  reverted: 'border-slate text-slate line-through',
}

export default function ProposalsTab({ circleId, proposals, onChanged }: Props) {
  const [expanded, setExpanded] = useState<string | null>(null)
  const [detail, setDetail] = useState<ProposalDetail | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const toggle = useCallback((proposalId: string) => {
    setActionError(null)
    if (expanded === proposalId) {
      setExpanded(null)
      setDetail(null)
      return
    }
    setExpanded(proposalId)
    setDetail(null)
    getProposalDetail(circleId, proposalId)
      .then(setDetail)
      .catch(e => setActionError(e.message))
  }, [circleId, expanded])

  const act = async (fn: typeof acceptProposal, proposalId: string) => {
    setActionError(null)
    setBusy(true)
    try {
      await fn(circleId, proposalId)
      setExpanded(null)
      setDetail(null)
      onChanged()
    } catch (e: any) {
      setActionError(e.message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex flex-col flex-1 min-h-0 overflow-hidden">
      <div className="section-header">
        <span>WORKSPACE CHANGES</span>
      </div>
      <div className="flex-1 overflow-y-auto p-4 flex flex-col gap-2 font-mono text-[11px]">
        {proposals.length === 0 && <div className="text-slate">NO CHANGES CAPTURED</div>}
        {actionError && <div className="file-error">{actionError}</div>}
        {proposals.map(p => (
          <div key={p.id} className="flex flex-col gap-1 pb-2 border-b border-dashed border-obsidian/20 last:border-0">
            <button onClick={() => toggle(p.id)} className="flex justify-between items-start gap-2 text-left w-full">
              <span className="font-bold leading-tight truncate" title={p.changed_paths.join(', ')}>
                {p.changed_paths.length === 1
                  ? p.changed_paths[0]
                  : `${p.changed_paths.length} files changed`}
              </span>
              <span className={`shrink-0 text-[9px] font-bold px-1 border ${STATUS_STYLE[p.status]}`}>
                {p.status.toUpperCase()}
              </span>
            </button>
            <div className="flex justify-between text-[9px] text-slate">
              <span>{p.actor_id ?? p.actor_hint ?? p.source}</span>
              <span>{age(p.created_at)}</span>
            </div>

            {expanded === p.id && (
              <div className="flex flex-col gap-2 mt-1">
                {!detail && !actionError && <div className="text-[9px] text-slate">LOADING…</div>}
                {detail?.files.map(f => (
                  <div key={f.path} className="border border-obsidian/30">
                    <div className="flex justify-between px-2 py-1 border-b border-dashed border-obsidian/20">
                      <span className="font-bold truncate" title={f.path}>{f.path}</span>
                      <span className="text-[9px] text-slate shrink-0">{f.change.toUpperCase()}</span>
                    </div>
                    {f.binary ? (
                      <div className="px-2 py-1 text-[9px] text-slate">BINARY</div>
                    ) : (
                      <div className="max-h-40 overflow-y-auto text-[10px] leading-snug">
                        {f.before !== null && f.change !== 'added' && (
                          <pre className="px-2 py-1 whitespace-pre-wrap break-all text-slate line-through decoration-slate/40 border-b border-dashed border-obsidian/10">{f.before}</pre>
                        )}
                        {f.after !== null && f.change !== 'removed' && (
                          <pre className="px-2 py-1 whitespace-pre-wrap break-all">{f.after}</pre>
                        )}
                      </div>
                    )}
                  </div>
                ))}
                {p.status === 'pending' && detail && (
                  <div className="flex gap-2">
                    <button disabled={busy} onClick={() => act(acceptProposal, p.id)}
                      className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster font-bold disabled:opacity-50">ACCEPT</button>
                    <button disabled={busy} onClick={() => act(rejectProposal, p.id)}
                      className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster font-bold disabled:opacity-50">REJECT</button>
                    <button disabled={busy} onClick={() => act(revertProposal, p.id)}
                      className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster font-bold disabled:opacity-50"
                      title="Restore all changed files to their pre-change state">REVERT</button>
                  </div>
                )}
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  )
}
