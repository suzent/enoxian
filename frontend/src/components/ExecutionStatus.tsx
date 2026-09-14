import { useEffect, useState } from 'react'
import { getExecutions, updateExecution } from '../api'
import type { ExecutionRun } from '../types'

/** Delivery is separate from message presence: seeing a post proves no run. */
export default function ExecutionStatus({ circleId }: { circleId: string }) {
  const [selfPeer, setSelfPeer] = useState<string | undefined>()
  const [runs, setRuns] = useState<ExecutionRun[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  useEffect(() => {
    let disposed = false
    setRuns([])
    const refresh = () => getExecutions(circleId).then(page => {
      if (!disposed) { setRuns(page.runs); setSelfPeer(page.peer_id); setError(null) }
    }).catch(() => { if (!disposed) setError('Delivery status unavailable') })
    void refresh()
    const timer = window.setInterval(refresh, 3000)
    return () => { disposed = true; window.clearInterval(timer) }
  }, [circleId])
  const act = async (run: ExecutionRun, action: 'retry' | 'cancel') => {
    if (action === 'retry' && !window.confirm('Retry this request? The previous attempt may already have changed files or performed actions.')) return
    setBusy(run.run_id)
    try {
      await updateExecution(circleId, run.run_id, action)
      setRuns((await getExecutions(circleId)).runs)
      setError(null)
    } catch (e) { setError(e instanceof Error ? e.message : String(e)) }
    finally { setBusy(null) }
  }
  if (!runs.length && !error) return null
  return <details className="border-t border-obsidian/15 px-3 py-2 text-xs">
    <summary>Agent delivery · {runs.filter(r => r.status === 'running').length} running · {runs.filter(r => r.status === 'pending').length} queued</summary>
    {error && <p role="status">{error}</p>}
    <p className="text-slate">Last reported execution states from each device. Messages without receipts may still be awaiting an offline recipient.</p>
    <ul className="max-h-40 overflow-auto">
      {runs.map(run => <li key={run.run_id} className="py-1 flex gap-2 flex-wrap">
        <a href={`#chat-message-${run.message_id}`}>@{run.agent_id}{run.peer_id ? ` · ${run.peer_id.slice(-8)}` : ''}</a>
        <span>{run.status === 'pending' ? 'queued / waiting for policy or capacity' : run.status}</span>
        {run.ambient && <span>ambient</span>}
        {run.detail && <span className="text-slate">{run.detail}</span>}
        {run.peer_id === selfPeer && run.status === 'pending' && <button disabled={busy !== null} onClick={() => void act(run, 'cancel')}>Cancel</button>}
        {run.peer_id === selfPeer && ['failed', 'interrupted', 'expired', 'cancelled', 'legacy_suppressed'].includes(run.status) && <button disabled={busy !== null} onClick={() => void act(run, 'retry')}>Retry</button>}
      </li>)}
    </ul>
  </details>
}
