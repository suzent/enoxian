import { useEffect, useRef, useState } from 'react'
import { getExecutions, updateExecution } from '../api'
import type { ChatMessage, ExecutionRun, Member } from '../types'

const retryable = new Set(['failed', 'interrupted', 'expired', 'cancelled'])
const statusLabels: Record<ExecutionRun['status'], string> = {
  pending: 'Waiting for a turn', running: 'Working', completed: 'Finished',
  failed: 'Couldn’t finish', interrupted: 'Stopped unexpectedly',
  expired: 'Not run', cancelled: 'Cancelled', legacy_suppressed: 'Outcome unavailable',
}

function normalizedAgent(run: ExecutionRun) {
  return run.agent_id.replace(/^~ambient:/, '')
}

/** Current work is prominent; imported history should never look like a queue. */
export default function ExecutionStatus({ onNavigate, circleId, members = [], messages = [] }: {
  onNavigate?: () => void; circleId: string; members?: Member[]; messages?: ChatMessage[]
}) {
  const [selfPeer, setSelfPeer] = useState<string | undefined>()
  const [runs, setRuns] = useState<ExecutionRun[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const generation = useRef(0)
  useEffect(() => {
    const current = ++generation.current
    let disposed = false
    let loading = false
    setRuns([]); setSelfPeer(undefined); setError(null); setBusy(null)
    const refresh = async () => {
      if (loading) return
      loading = true
      try {
        const page = await getExecutions(circleId)
        if (!disposed && generation.current === current) {
          setRuns(page.runs); setSelfPeer(page.peer_id); setError(null)
        }
      } catch {
        if (!disposed) setError('Could not refresh agent activity. Showing the last reported status.')
      } finally { loading = false }
    }
    void refresh()
    const timer = window.setInterval(refresh, 3000)
    return () => { disposed = true; generation.current++; window.clearInterval(timer) }
  }, [circleId])
  const act = async (run: ExecutionRun, action: 'retry' | 'cancel') => {
    if (action === 'retry' && !window.confirm('Run this request again? The earlier attempt may already have changed files or performed actions.')) return
    const current = generation.current
    setBusy(run.run_id)
    try {
      await updateExecution(circleId, run.run_id, action)
      const page = await getExecutions(circleId)
      if (generation.current === current) { setRuns(page.runs); setSelfPeer(page.peer_id); setError(null) }
    } catch (e) {
      if (generation.current === current) setError(e instanceof Error ? e.message : String(e))
    } finally { if (generation.current === current) setBusy(null) }
  }
  const active = runs.filter(r => r.status === 'running' || r.status === 'pending')
  const attention = runs.filter(r => r.status === 'failed' || r.status === 'interrupted' || r.status === 'expired')
  const history = runs.filter(r => r.status === 'completed' || r.status === 'cancelled')
  const rows = (items: ExecutionRun[]) => <ul className="agent-activity__list">
    {items.map(run => {
      const agent = normalizedAgent(run)
      const host = members.find(m => m.peer_id === run.peer_id)
      const local = !!selfPeer && run.peer_id === selfPeer
      const device = host?.device_label || (local ? 'This device' : run.peer_id ? `Device ${run.peer_id.slice(-8)}` : 'Unknown device')
      const source = messages.find(m => m.id === run.message_id)
      const ambient = run.ambient || run.agent_id.startsWith('~ambient:')
      const label = run.status === 'completed' && run.detail === 'No reply needed' ? 'No reply needed' : statusLabels[run.status]
      return <li key={run.run_id} className={`agent-activity__item agent-activity__item--${run.status}`}>
        <div className="agent-activity__identity">
          <strong className="agent-activity__name">@{agent}</strong>
          <span className="agent-activity__status">{label}</span>
        </div>
        <div className="agent-activity__device" title={host?.owner ? `${host.owner} · ${device}` : device}>{device}{local && host?.device_label ? ' · this device' : ''}</div>
        <div className="agent-activity__meta">{ambient ? 'Read the room' : 'Addressed request'}
          {run.updated_at != null && <time dateTime={new Date(run.updated_at * 1000).toISOString()} title="Last status update from this device">{new Date(run.updated_at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time>}

        </div>
        {source ? <a className="agent-activity__source" href={`#chat-message-${run.message_id}`} onClick={onNavigate} title={source.text}>{source.text}</a>
          : <span className="agent-activity__unavailable">Message unavailable</span>}
        {run.detail && run.detail !== label && <p className="agent-activity__detail">{run.detail}</p>}
        <div className="agent-activity__actions">
          {local && run.status === 'pending' && <button type="button" className="underline underline-offset-2" disabled={busy !== null} onClick={() => void act(run, 'cancel')}>{busy === run.run_id ? 'Updating…' : 'Cancel request'}</button>}
          {local && retryable.has(run.status) && <button type="button" className="underline underline-offset-2" disabled={busy !== null} onClick={() => void act(run, 'retry')}>{busy === run.run_id ? 'Updating…' : 'Try again'}</button>}
          {!local && retryable.has(run.status) && <span className="text-slate">Retry on {device}.</span>}
        </div>
      </li>
    })}
  </ul>
  return <section aria-label="Agent activity" className="agent-activity">
    <header className="agent-activity__header">
      <h3>Agent activity</h3>
      <span className="agent-activity__count" title="Requests working or waiting">{active.length}</span>
    </header>
    {error && <p role="status" className="agent-activity__error">{error}</p>}
    {active.length > 0 ? <>
      <p className="agent-activity__summary">{active.filter(r => r.status === 'running').length} working · {active.filter(r => r.status === 'pending').length} waiting</p>
      {rows(active)}
    </> : <p className="agent-activity__empty">No active requests</p>}
    {attention.length > 0 && <details className="agent-activity__group"><summary>Needs attention <span>{attention.length}</span></summary>{rows(attention)}</details>}
    {history.length > 0 && <details className="agent-activity__group"><summary>Recent history <span>{history.length}</span></summary>{rows([...history].sort((a, b) => (b.updated_at ?? 0) - (a.updated_at ?? 0)))}</details>}
    <p className="agent-activity__note" title="Status is reported by each device and may be out of date while it’s disconnected. A missing entry doesn’t confirm delivery.">Last reported by each device <span aria-hidden="true">ⓘ</span></p>
  </section>
}
