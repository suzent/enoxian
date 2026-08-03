import { useState, useCallback, useEffect, useRef } from 'react'
import type { Proposal, ProposalDetail, ProposalFileDiff } from '../types'
import { getProposalDetail, acceptProposal, rejectProposal, revertProposal } from '../api'
import { lineDiff, collapseContext } from '../lib/lineDiff'

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

const STATUS_STYLE: Record<Proposal['status'], { pill: string; dot: string }> = {
  pending:    { pill: 'border-obsidian bg-obsidian text-alabaster', dot: 'bg-obsidian' },
  accepted:   { pill: 'border-obsidian text-obsidian',              dot: 'bg-obsidian' },
  synced:     { pill: 'border-obsidian text-obsidian',              dot: 'bg-obsidian' },
  conflicted: { pill: 'border-amber-600 text-amber-700',            dot: 'bg-amber-500' },
  rejected:   { pill: 'border-slate/40 text-slate/60',              dot: 'bg-slate/30' },
  reverted:   { pill: 'border-slate/40 text-slate/60',              dot: 'bg-slate/30' },
}

export default function ProposalsTab({ circleId, proposals, onChanged }: Props) {
  const [expanded, setExpanded] = useState<string | null>(null)
  const [detail, setDetail] = useState<ProposalDetail | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const circleIdRef = useRef(circleId)
  const detailRequestRef = useRef(0)
  circleIdRef.current = circleId

  useEffect(() => {
    detailRequestRef.current += 1
    setExpanded(null)
    setDetail(null)
    setActionError(null)
    setBusy(false)
  }, [circleId])

  const toggle = useCallback((proposalId: string) => {
    setActionError(null)
    if (expanded === proposalId) {
      setExpanded(null)
      setDetail(null)
      return
    }
    setExpanded(proposalId)
    setDetail(null)
    const requestedCircleId = circleId
    const requestId = ++detailRequestRef.current
    getProposalDetail(requestedCircleId, proposalId)
      .then(data => {
        if (circleIdRef.current === requestedCircleId && detailRequestRef.current === requestId) {
          setDetail(data)
        }
      })
      .catch(e => {
        if (circleIdRef.current === requestedCircleId && detailRequestRef.current === requestId) {
          setActionError(e.message)
        }
      })
  }, [circleId, expanded])

  const act = async (fn: typeof acceptProposal, proposalId: string) => {
    const requestedCircleId = circleId
    setActionError(null)
    setBusy(true)
    try {
      await fn(requestedCircleId, proposalId)
      if (circleIdRef.current === requestedCircleId) {
        setExpanded(null)
        setDetail(null)
        onChanged()
      }
    } catch (e: any) {
      if (circleIdRef.current === requestedCircleId) setActionError(e.message)
    } finally {
      if (circleIdRef.current === requestedCircleId) setBusy(false)
    }
  }

  return (
    <div className="flex flex-col flex-1 min-h-0 overflow-hidden">
      <div className="section-header">
        <span>WORKSPACE CHANGES</span>
      </div>
      <div className="flex-1 overflow-y-auto px-3 py-3 flex flex-col gap-2 font-mono text-[11px]">
        {proposals.length === 0 && <div className="text-slate px-1">NO CHANGES CAPTURED</div>}
        {actionError && <div className="file-error">{actionError}</div>}
        {proposals.map(p => {
          const st = STATUS_STYLE[p.status]
          const isExpanded = expanded === p.id
          const actor = p.actor_id ?? p.actor_hint ?? p.source
          const label = p.changed_paths.length === 1
            ? p.changed_paths[0]
            : `${p.changed_paths.length} files`
          return (
            <div key={p.id} className={`border border-obsidian/25 ${isExpanded ? 'border-obsidian/60' : 'hover:border-obsidian/45'}`}>
              {/* Card header — always visible */}
              <button
                onClick={() => toggle(p.id)}
                className="w-full flex items-center gap-2 px-2.5 py-2 text-left"
              >
                {/* Status dot */}
                <span className={`shrink-0 w-1.5 h-1.5 rounded-full ${st.dot}`} aria-hidden="true" />
                {/* Filename(s) */}
                <span className="flex-1 font-bold truncate min-w-0" title={p.changed_paths.join(', ')}>
                  {label}
                </span>
                {/* Status pill */}
                <span className={`shrink-0 text-[8px] font-bold tracking-wide px-1.5 py-0.5 border ${st.pill}`}>
                  {p.status.toUpperCase()}
                </span>
              </button>

              {/* Meta row */}
              <div className="flex items-center justify-between px-2.5 pb-2 text-[9px] text-slate/70">
                <span className="truncate min-w-0" title={p.origin_peer_id || undefined}>
                  {actor}{p.origin_device ? <span className="text-slate/40"> @ {p.origin_device}</span> : ''}
                </span>
                <span className="shrink-0 ml-2">{age(p.created_at)}</span>
              </div>

              {/* Expanded diff + actions */}
              {isExpanded && (
                <div className="border-t border-dashed border-obsidian/20 flex flex-col gap-2 p-2.5">
                  {!detail && !actionError && <div className="text-[9px] text-slate">LOADING…</div>}
                  {detail?.files.map(f => (
                    <div key={f.path} className="border border-obsidian/30">
                      <div className="flex justify-between px-2 py-1 bg-obsidian/4 border-b border-dashed border-obsidian/20">
                        <span className="font-bold truncate text-[10px]" title={f.path}>{f.path}</span>
                        <span className="text-[8px] text-slate shrink-0 ml-2">{f.change.toUpperCase()}</span>
                      </div>
                      {f.binary ? (
                        <div className="px-2 py-1 text-[9px] text-slate">BINARY</div>
                      ) : (
                        <FileDiffView file={f} />
                      )}
                    </div>
                  ))}
                  {p.status === 'pending' && detail && (
                    <div className="flex gap-1.5 pt-0.5">
                      <button disabled={busy} onClick={() => act(acceptProposal, p.id)}
                        className="text-[9px] border border-obsidian px-2.5 py-1 hover:bg-obsidian hover:text-alabaster font-bold disabled:opacity-50"
                        title="Keep these changes">ACCEPT</button>
                      <button disabled={busy} onClick={() => act(rejectProposal, p.id)}
                        className="text-[9px] border border-obsidian/50 px-2.5 py-1 hover:bg-obsidian hover:text-alabaster font-bold disabled:opacity-50 text-obsidian/70"
                        title="Restore all changed files to their pre-change state">REJECT</button>
                    </div>
                  )}
                  {p.status === 'accepted' && detail && (
                    <div className="flex gap-1.5 pt-0.5">
                      <button disabled={busy} onClick={() => act(revertProposal, p.id)}
                        className="text-[9px] border border-obsidian/50 px-2.5 py-1 hover:bg-obsidian hover:text-alabaster font-bold disabled:opacity-50 text-obsidian/70"
                        title="Undo this accepted change">REVERT</button>
                    </div>
                  )}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}

// ── Per-file line diff with gutters and coloring ──────────────────────────────

const ROW_STYLE: Record<'context' | 'add' | 'del', { row: string; marker: string }> = {
  context: { row: '', marker: ' ' },
  add: { row: 'bg-green-600/15 text-green-900', marker: '+' },
  del: { row: 'bg-red-600/15 text-red-900 line-through decoration-red-900/30', marker: '-' },
}

function FileDiffView({ file }: { file: ProposalFileDiff }) {
  const rows = collapseContext(lineDiff(file.before, file.after))

  return (
    <div className="max-h-48 overflow-y-auto text-[10px] leading-snug font-mono">
      <table className="w-full border-collapse">
        <tbody>
          {rows.map((row, idx) => {
            if (row.type === 'skip') {
              return (
                <tr key={idx} className="text-slate/60">
                  <td colSpan={4} className="px-2 py-0.5 border-y border-dashed border-obsidian/10 text-center select-none">
                    ··· {row.text} unchanged line{row.text === '1' ? '' : 's'} ···
                  </td>
                </tr>
              )
            }
            const style = ROW_STYLE[row.type]
            return (
              <tr key={idx} className={style.row}>
                <td className="w-7 pr-1 text-right text-slate/50 select-none border-r border-obsidian/10 align-top">
                  {row.oldLine ?? ''}
                </td>
                <td className="w-7 pr-1 text-right text-slate/50 select-none border-r border-obsidian/15 align-top">
                  {row.newLine ?? ''}
                </td>
                <td className="w-3 text-center select-none font-bold align-top">{style.marker}</td>
                <td className="px-1 whitespace-pre-wrap break-all align-top">{row.text || ' '}</td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}
