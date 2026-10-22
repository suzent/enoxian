import { useEffect, useRef, useState } from 'react'
import { getExecutions, getWardenDecisions, updateExecution, type WardenRecord } from '../api'
import { describeDecision } from './WardenSettings'
import type { AdmissionSkip, ChatMessage, ExecutionRun, Member, Readiness, RunActivity, RunSpend, RunUsage } from '../types'

const retryable = new Set(['failed', 'interrupted', 'expired', 'cancelled'])
const statusLabels: Record<ExecutionRun['status'], string> = {
  pending: 'Waiting for a turn', running: 'Working', completed: 'Finished',
  failed: 'Couldn’t finish', interrupted: 'Stopped unexpectedly',
  expired: 'Not run', cancelled: 'Cancelled', legacy_suppressed: 'Outcome unavailable',
}

function ago(secs: number) {
  const s = Math.max(0, Math.floor(secs))
  if (s < 60) return `${s}s`
  if (s < 3600) return `${Math.floor(s / 60)}m`
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`
}

/** How long a turn can say nothing, with no tool call open, before the panel
 *  mentions it. Only a hint for whoever is looking: nothing is stopped. */
export const QUIET_HINT_SECS = 10 * 60

/** One line on what a running turn is doing: its open tool calls, or how long
 *  it has been quiet. */
export function describeActivity(activity: RunActivity, now: number): { text: string; quiet: boolean } {
  if (activity.stopping) return { text: 'Stopping…', quiet: false }
  const running = `Running ${ago(now - activity.started_at)}`
  if (activity.open_tools.length > 0) {
    const tools = activity.open_tools.map(t => `${t.title} (${ago(now - t.since)})`).join(', ')
    return { text: `${running} · using ${tools}`, quiet: false }
  }
  const quiet = activity.idle_secs >= QUIET_HINT_SECS
  return {
    text: quiet
      ? `${running} · nothing reported for ${ago(activity.idle_secs)}. It may be stuck; stop it if so.`
      : `${running} · last activity ${ago(activity.idle_secs)} ago`,
    quiet,
  }
}

function money(amount: number, currency: string) {
  try {
    return new Intl.NumberFormat(undefined, { style: 'currency', currency, minimumFractionDigits: 2, maximumFractionDigits: amount < 1 ? 4 : 2 }).format(amount)
  } catch {
    return `${amount.toFixed(4)} ${currency}`
  }
}

function tokens(n: number) {
  if (n >= 1_000_000) return `${Number((n / 1_000_000).toFixed(1))}M`
  return n >= 1000 ? `${Math.round(n / 1000)}k` : String(n)
}

/** One line on what a finished or running turn used, as its agent reported it. */
export function describeUsage(usage: RunUsage): string {
  const context = usage.context_size > 0 ? `context ${tokens(usage.context_used)} / ${tokens(usage.context_size)}` : undefined
  const cost = usage.cost ? money(usage.cost.amount, usage.cost.currency) : 'cost not reported'
  const used = usage.tokens ? `${tokens(usage.tokens.total)} tokens` : undefined
  return [cost, used, context].filter(Boolean).join(' · ')
}

const REPORTED = 'As reported by the agent, not a bill: on a subscription it may be what the turn would cost at API prices. Only this device sees it.'

function usageTitle(usage: RunUsage): string {
  const t = usage.tokens
  if (!t) return REPORTED
  return `${t.input} input, ${t.output} output, ${t.cached_read} read from cache, ${t.cached_write} written to cache. ${REPORTED}`
}

/** Today's spend on this device, scrying vs addressed. `spend` is every run
 *  this device executed in the last two days, straight from its own inbox, so
 *  the total is not limited to the runs listed. Turns whose agent reported no
 *  cost are counted apart rather than as free. */
export function usageToday(spend: RunSpend[], now: Date): string | undefined {
  const midnight = new Date(now); midnight.setHours(0, 0, 0, 0)
  const today = spend.filter(s => s.updated_at >= midnight.getTime() / 1000)
  if (today.length === 0) return undefined
  const part = (ambient: boolean, label: string) => {
    const group = today.filter(s => s.ambient === ambient)
    if (group.length === 0) return undefined
    const sums = new Map<string, number>()
    for (const { usage: { cost } } of group) {
      if (cost) sums.set(cost.currency, (sums.get(cost.currency) ?? 0) + cost.amount)
    }
    const total = [...sums].map(([currency, amount]) => money(amount, currency)).join(' + ') || 'no cost reported'
    return `${label} ${total} (${group.length} ${group.length === 1 ? 'turn' : 'turns'})`
  }
  const unpriced = today.filter(s => !s.usage.cost).length
  return [
    part(true, 'scrying'),
    part(false, 'addressed'),
    unpriced > 0 ? `${unpriced} without a cost` : undefined,
  ].filter(Boolean).join(' · ')
}

function normalizedAgent(run: ExecutionRun) {
  return run.agent_id.replace(/^~ambient:/, '')
}

/** The standing reasons nothing can happen here, as opposed to the per-message
 *  ones below. Configuration is invisible from the transcript, and an empty
 *  listener list is the most common cause of "the agent wasn't triggered". */
function readinessWarnings(readiness?: Readiness): string[] {
  if (!readiness) return []
  const warnings: string[] = []
  if (readiness.reaction !== 'push')
    warnings.push('This device is set to pull, so it never launches an agent on its own.')
  if (readiness.ambient.length === 0 && readiness.engagement_window_secs <= 0)
    warnings.push('No agent scries this room, and follow-ups are off — only an @mention reaches an agent here.')
  else if (readiness.ambient.length === 0)
    warnings.push('No agent scries this room. A message that names nobody only routes as a follow-up.')
  if (readiness.ambient_unconfigured.length > 0)
    warnings.push(`Listed as scrying the room but not configured on this device, so ignored: ${readiness.ambient_unconfigured.join(', ')}.`)
  return warnings
}

/** Current work is prominent; imported history should never look like a queue. */
export default function ExecutionStatus({ onNavigate, circleId, members = [], messages = [] }: {
  onNavigate?: () => void; circleId: string; members?: Member[]; messages?: ChatMessage[]
}) {
  const [selfPeer, setSelfPeer] = useState<string | undefined>()
  const [runs, setRuns] = useState<ExecutionRun[]>([])
  const [activity, setActivity] = useState<Record<string, RunActivity>>({})
  const [usage, setUsage] = useState<Record<string, RunUsage>>({})
  const [spend, setSpend] = useState<RunSpend[]>([])
  const [skips, setSkips] = useState<AdmissionSkip[]>([])
  const [readiness, setReadiness] = useState<Readiness | undefined>()
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const generation = useRef(0)
  // The warden's records, by message: its latest decision, and how a vote went.
  // Polled slower than runs: decisions are made once per message.
  const [warden, setWarden] = useState<{ decided: Record<string, WardenRecord>; votes: Record<string, WardenRecord> }>({ decided: {}, votes: {} })
  useEffect(() => {
    let disposed = false
    setWarden({ decided: {}, votes: {} })
    const refresh = () => getWardenDecisions(circleId, 100).then(page => {
      if (disposed) return
      const decided: Record<string, WardenRecord> = {}
      const votes: Record<string, WardenRecord> = {}
      // Newest first: keep the first seen for each message.
      for (const record of page.records) {
        if (record.vote) { votes[record.message_id] ??= record }
        else if (record.decision && record.phase !== 'sampled') decided[record.message_id] ??= record
      }
      setWarden({ decided, votes })
    }).catch(() => {})
    void refresh()
    const timer = window.setInterval(refresh, 10_000)
    return () => { disposed = true; window.clearInterval(timer) }
  }, [circleId])
  useEffect(() => {
    const current = ++generation.current
    let disposed = false
    let loading = false
    setRuns([]); setActivity({}); setUsage({}); setSpend([]); setSkips([]); setReadiness(undefined); setSelfPeer(undefined); setError(null); setBusy(null)
    const refresh = async () => {
      if (loading) return
      loading = true
      try {
        const page = await getExecutions(circleId)
        if (!disposed && generation.current === current) {
          setRuns(page.runs); setActivity(page.activity ?? {}); setUsage(page.usage ?? {}); setSpend(page.spend ?? []); setSkips(page.skips ?? []); setReadiness(page.readiness)
          setSelfPeer(page.peer_id); setError(null)
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
    if (action === 'cancel' && run.status === 'running' && !window.confirm('Stop this turn? The agent is asked to wind down and posts nothing; anything it already changed stays.')) return
    const current = generation.current
    setBusy(run.run_id)
    try {
      await updateExecution(circleId, run.run_id, action)
      const page = await getExecutions(circleId)
      if (generation.current === current) {
        setRuns(page.runs); setActivity(page.activity ?? {}); setUsage(page.usage ?? {}); setSpend(page.spend ?? []); setSkips(page.skips ?? []); setReadiness(page.readiness)
        setSelfPeer(page.peer_id); setError(null)
      }
    } catch (e) {
      if (generation.current === current) setError(e instanceof Error ? e.message : String(e))
    } finally { if (generation.current === current) setBusy(null) }
  }
  const active = runs.filter(r => r.status === 'running' || r.status === 'pending')
  // A message the device has since picked up again, or already answered. Its
  // earlier failures are history, not something anyone needs to act on: a turn
  // killed by a restart and automatically put back would otherwise sit under
  // "Needs attention" offering a "Try again" that has already happened, which
  // makes a successful recovery read as an outstanding failure.
  const movedOn = new Set(
    runs.filter(r => r.status === 'completed' || r.status === 'running' || r.status === 'pending')
      .map(r => r.message_id),
  )
  const failed = (r: ExecutionRun) =>
    r.status === 'failed' || r.status === 'interrupted' || r.status === 'expired'
  const attention = runs.filter(r => failed(r) && !movedOn.has(r.message_id))
  // Listed explicitly rather than as "everything else": an imported marker is
  // shown nowhere at all, and a catch-all would surface it as history.
  const history = runs.filter(
    r => r.status === 'completed' || r.status === 'cancelled' || (failed(r) && movedOn.has(r.message_id)),
  )
  // Retrying a run the device has already replaced just starts a duplicate
  // turn on a question that is being answered.
  const canRetry = (r: ExecutionRun) => retryable.has(r.status) && !movedOn.has(r.message_id)
  const rows = (items: ExecutionRun[]) => <ul className="agent-activity__list">
    {items.map(run => {
      const agent = normalizedAgent(run)
      const host = members.find(m => m.peer_id === run.peer_id)
      const local = !!selfPeer && run.peer_id === selfPeer
      const device = host?.device_label || (local ? 'This device' : run.peer_id ? `Device ${run.peer_id.slice(-8)}` : 'Unknown device')
      const source = messages.find(m => m.id === run.message_id)
      const ambient = run.ambient || run.agent_id.startsWith('~ambient:')
      const label = run.status === 'completed' && run.detail === 'No reply needed' ? 'No reply needed' : statusLabels[run.status]
      const running = activity[run.run_id]
      const live = running ? describeActivity(running, Date.now() / 1000) : undefined
      return <li key={run.run_id} className={`agent-activity__item agent-activity__item--${run.status}`}>
        <div className="agent-activity__identity">
          <strong className="agent-activity__name">@{agent}</strong>
          <span className="agent-activity__status">{label}</span>
        </div>
        <div className="agent-activity__device" title={host?.owner ? `${host.owner} · ${device}` : device}>{device}{local && host?.device_label ? ' · this device' : ''}</div>
        <div className="agent-activity__meta">{ambient ? 'Scrying the room' : 'Addressed request'}
          {run.updated_at != null && <time dateTime={new Date(run.updated_at * 1000).toISOString()} title="Last status update from this device">{new Date(run.updated_at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time>}

        </div>
        {source ? <a className="agent-activity__source" href={`#chat-message-${run.message_id}`} onClick={onNavigate} title={source.text}>{source.text}</a>
          : <span className="agent-activity__unavailable">Message unavailable</span>}
        {run.status === 'running' && live && <p className={`agent-activity__live${live.quiet ? ' agent-activity__live--quiet' : ''}`}>{live.text}</p>}
        {run.detail && run.detail !== label && <p className="agent-activity__detail">{run.detail}</p>}
        {ambient && warden.decided[run.message_id]?.decision && <p className="agent-activity__warden" title={warden.decided[run.message_id].decision!.decider}>
          {warden.decided[run.message_id].phase === 'shadow' ? 'Warden would have chosen: ' : 'Warden: '}
          {describeDecision(warden.decided[run.message_id].decision!)}
        </p>}
        {ambient && warden.votes[run.message_id]?.vote && <p className="agent-activity__warden">
          Blind vote: {warden.votes[run.message_id].vote!.groups.length === 1 ? 'all agreed' : `${warden.votes[run.message_id].vote!.groups.length} different answers`}
          {warden.votes[run.message_id].vote!.posted_by ? ` · posted @${warden.votes[run.message_id].vote!.posted_by}'s` : warden.votes[run.message_id].vote!.added ? ` · asked @${warden.votes[run.message_id].vote!.added} too` : ''}
        </p>}
        {local && usage[run.run_id] && <p className="agent-activity__usage" title={usageTitle(usage[run.run_id])}>{describeUsage(usage[run.run_id])}</p>}
        <div className="agent-activity__actions">
          {local && run.status === 'running' && running && <button type="button" className="underline underline-offset-2" disabled={busy !== null || running.stopping} onClick={() => void act(run, 'cancel')}>{running.stopping ? 'Stopping…' : busy === run.run_id ? 'Updating…' : 'Stop'}</button>}
          {local && run.status === 'pending' && <button type="button" className="underline underline-offset-2" disabled={busy !== null} onClick={() => void act(run, 'cancel')}>{busy === run.run_id ? 'Updating…' : 'Cancel request'}</button>}
          {local && canRetry(run) && <button type="button" className="underline underline-offset-2" disabled={busy !== null} onClick={() => void act(run, 'retry')}>{busy === run.run_id ? 'Updating…' : 'Try again'}</button>}
          {!local && canRetry(run) && <span className="text-slate">Retry on {device}.</span>}
        </div>
      </li>
    })}
  </ul>
  const warnings = readinessWarnings(readiness)
  const spent = usageToday(spend, new Date())
  // A run that exists is a better answer than a reason it might not have, so a
  // message with either is only ever listed once, under its run.
  const ran = new Set(runs.map(r => r.message_id))
  const notPickedUp = skips.filter(s => !ran.has(s.message_id))
  return <section aria-label="Agent activity" className="agent-activity">
    <header className="agent-activity__header">
      <h3>Agent activity</h3>
      <span className="agent-activity__count" title="Requests working or waiting">{active.length}</span>
    </header>
    {error && <p role="status" className="agent-activity__error">{error}</p>}
    {warnings.map(warning => <p key={warning} className="agent-activity__readiness">{warning}</p>)}
    {spent && <p className="agent-activity__spend" title="What this device's agents reported spending since midnight — their figures, not a bill; on a subscription they may be API-price equivalents. Agents that report no cost are counted apart, not as free. Only this device sees it.">Today on this device: {spent}</p>}
    {active.length > 0 ? <>
      <p className="agent-activity__summary">{active.filter(r => r.status === 'running').length} working · {active.filter(r => r.status === 'pending').length} waiting</p>
      {rows(active)}
    </> : <p className="agent-activity__empty">No active requests</p>}
    {attention.length > 0 && <details className="agent-activity__group"><summary>Needs attention <span>{attention.length}</span></summary>{rows(attention)}</details>}
    {history.length > 0 && <details className="agent-activity__group"><summary>Recent history <span>{history.length}</span></summary>{rows([...history].sort((a, b) => (b.updated_at ?? 0) - (a.updated_at ?? 0)))}</details>}
    {notPickedUp.length > 0 && <details className="agent-activity__group">
      <summary title="Messages this device saw and chose not to act on">Not picked up <span>{notPickedUp.length}</span></summary>
      <ul className="agent-activity__list">
        {notPickedUp.map(skip => {
          const source = messages.find(m => m.id === skip.message_id)
          return <li key={`${skip.message_id}:${skip.agent ?? ''}:${skip.reason}`} className="agent-activity__item agent-activity__item--skipped">
            <div className="agent-activity__identity">
              <strong className="agent-activity__name">{skip.agent ? `@${skip.agent}` : 'Nobody'}</strong>
              <span className="agent-activity__status">Didn’t run</span>
            </div>
            <p className="agent-activity__detail">{skip.reason}</p>
            {source ? <a className="agent-activity__source" href={`#chat-message-${skip.message_id}`} onClick={onNavigate} title={source.text}>{source.text}</a>
              : <span className="agent-activity__unavailable">Message unavailable</span>}
            <div className="agent-activity__meta">
              {skip.count > 1 ? `Still true after ${skip.count} checks` : 'Decided once'}
              <time dateTime={new Date(skip.last_at * 1000).toISOString()} title="Last time this device re-checked">{new Date(skip.last_at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time>
            </div>
          </li>
        })}
      </ul>
    </details>}
    <p className="agent-activity__note" title="Status is reported by each device and may be out of date while it’s disconnected. A missing entry doesn’t confirm delivery. “Not picked up” is this device’s own reasoning and is cleared when the daemon restarts.">Last reported by each device <span aria-hidden="true">ⓘ</span></p>
  </section>
}
