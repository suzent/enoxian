import { useEffect, useRef, useState } from 'react'
import { Laptop } from 'lucide-react'
import { getExecutions, getWardenDecisions, updateExecution, type WardenRecord } from '../api'
import { describeDecision } from './WardenSettings'
import { brokenAgents, healthKey } from '../lib/agentHealth'
import AgentFixBadge from './AgentFixBadge'
import type { AdmissionSkip, ChatMessage, ExecutionRun, Member, Readiness, RunActivity, RunSpend, RunUsage } from '../types'
import AgentIcon from './AgentIcon'

const retryable = new Set(['failed', 'interrupted', 'expired', 'cancelled'])
const statusLabels: Record<ExecutionRun['status'], string> = {
  pending: 'Queued', running: 'Working', completed: 'Answered',
  failed: 'Failed', interrupted: 'Interrupted',
  expired: 'Not run', cancelled: 'Cancelled', legacy_suppressed: 'Unknown',
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

/** How long a failed turn stays under "Failed · can retry" before it is
 *  filed under Done. */
export const RETRY_WINDOW_SECS = 24 * 60 * 60

/** Context-window share (%) from which a running turn shows its meter. */
const CONTEXT_BAR_FROM = 50

/** Human-readable label for the latest ACP event when no tool call is open. */
export function describePhase(lastKind?: string | null, lastFile?: string | null): string | undefined {
  switch (lastKind) {
    case 'agent_message_chunk':
    case 'agent_message':
      return 'writing reply'
    case 'agent_thought_chunk':
    case 'agent_thought':
      return 'thinking'
    case 'fs/read_text_file':
      return lastFile ? `reading ${lastFile}` : 'reading file'
    case 'fs/write_text_file':
      return lastFile ? `writing ${lastFile}` : 'writing file'
    case 'session/request_permission':
      return 'requesting permission'
    case 'plan':
      return 'updating plan'
    default:
      return undefined
  }
}

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
  const phase = describePhase(activity.last_kind, activity.last_file)
  return {
    text: quiet
      ? `${running} · nothing reported for ${ago(activity.idle_secs)}. It may be stuck; stop it if so.`
      : phase
        ? `${running} · ${phase} · ${ago(activity.idle_secs)} ago`
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
  // An agent that reports no cost says nothing here: "cost not reported" on
  // every turn read as an error. The daily total still counts such turns.
  const cost = usage.cost ? money(usage.cost.amount, usage.cost.currency) : undefined
  const used = usage.tokens ? `${tokens(usage.tokens.total)} tokens` : undefined
  return [cost, used, context].filter(Boolean).join(' · ')
}

const REPORTED = 'As reported by the agent, not a bill: on a subscription it may be what the turn would cost at API prices. Only this device sees it.'

function usageTitle(usage: RunUsage): string {
  const t = usage.tokens
  if (!t) return REPORTED
  return `${t.input} input, ${t.output} output, ${t.cached_read} read from cache, ${t.cached_write} written to cache. ${REPORTED}`
}

/** Today's reported spend on this device as one short figure, for the stats
 *  row; the breakdown is {@link usageToday}. */
export function todayCost(spend: RunSpend[], now: Date): string | undefined {
  const midnight = new Date(now); midnight.setHours(0, 0, 0, 0)
  const sums = new Map<string, number>()
  for (const s of spend) {
    if (s.updated_at < midnight.getTime() / 1000 || !s.usage.cost) continue
    sums.set(s.usage.cost.currency, (sums.get(s.usage.cost.currency) ?? 0) + s.usage.cost.amount)
  }
  if (sums.size === 0) return undefined
  return [...sums].map(([currency, amount]) => money(amount, currency)).join(' + ')
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
export default function ExecutionStatus({ onNavigate, onOpenMembers, circleId, members = [], messages = [] }: {
  onNavigate?: () => void; onOpenMembers?: () => void; circleId: string; members?: Member[]; messages?: ChatMessage[]
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
  // Runs whose tool-call list is open. Collapsed by default: the live line
  // already says what the agent is doing now, and the list is for digging in.
  const [openTools, setOpenTools] = useState<Set<string>>(new Set())
  const toggleTools = (runId: string) => setOpenTools(prev => {
    const next = new Set(prev)
    if (next.has(runId)) next.delete(runId); else next.add(runId)
    return next
  })
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
  // "Failed" offering a "Try again" that has already happened, which
  // makes a successful recovery read as an outstanding failure.
  const movedOn = new Set(
    runs.filter(r => r.status === 'completed' || r.status === 'running' || r.status === 'pending')
      .map(r => r.message_id),
  )
  const failed = (r: ExecutionRun) =>
    r.status === 'failed' || r.status === 'interrupted' || r.status === 'expired'
  // An agent that is broken on its device fails every message the same way.
  // Those failures are one fact about the agent, said once below and in
  // Members, not one card per message here.
  const broken = brokenAgents(runs)
  const agentFault = (r: ExecutionRun) => {
    const b = broken.get(healthKey(r.peer_id, normalizedAgent(r)))
    // Every failure since it last worked: they all wait on the same fix.
    return failed(r) && !!b && (r.updated_at ?? 0) >= b.since
  }
  // A failure from days ago is history, not a to-do: the conversation has
  // moved on, and retrying it now would answer a question nobody is waiting on.
  const stale = (r: ExecutionRun) => r.updated_at != null && r.updated_at < Date.now() / 1000 - RETRY_WINDOW_SECS
  const attention = runs.filter(r => failed(r) && !movedOn.has(r.message_id) && !agentFault(r) && !stale(r))
  // Listed explicitly rather than as "everything else": an imported marker is
  // shown nowhere at all, and a catch-all would surface it as history.
  const history = runs.filter(
    r => r.status === 'completed' || r.status === 'cancelled' || (failed(r) && (movedOn.has(r.message_id) || agentFault(r) || stale(r))),
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
      const label = run.status === 'completed' && run.detail === 'No reply needed' ? 'No reply' : statusLabels[run.status]
      const running = activity[run.run_id]
      const live = running ? describeActivity(running, Date.now() / 1000) : undefined
      return <li key={run.run_id} className={`agent-activity__item agent-activity__item--${run.status}`}>
        <div className="agent-activity__identity">
          <span className="agent-activity__who">
            <AgentIcon name={agent} className="agent-activity__bot" />
            <strong className="agent-activity__name">@{agent}</strong>
          </span>
          <span className={`agent-activity__status agent-activity__status--${run.status}`}>{label}</span>
        </div>
        <div className="agent-activity__meta">
          <span className="agent-activity__device" title={`Device: ${host?.owner ? `${host.owner} · ${device}` : device}`}><Laptop size={11} aria-hidden="true" />{device}{local && host?.device_label ? ' (this one)' : ''}</span>
          <span title={ambient ? 'Nobody named this agent: it picked the message up from the room' : 'Someone @mentioned this agent'}>{ambient ? 'from room' : 'mentioned'}</span>
          {run.updated_at != null && <time dateTime={new Date(run.updated_at * 1000).toISOString()} title="Last status update from this device">{new Date(run.updated_at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time>}
        </div>
        {source ? <a className="agent-activity__source" href={`#chat-message-${run.message_id}`} onClick={onNavigate} title={source.text}>{source.text}</a>
          : <span className="agent-activity__unavailable">Message unavailable</span>}
        {run.status === 'running' && live && <p className={`agent-activity__live${live.quiet ? ' agent-activity__live--quiet' : ''}`}>{live.text}</p>}
        {run.status === 'running' && running?.plan && running.plan.length > 0 && <ul className="agent-activity__plan" aria-label="Agent plan">
          {running.plan.map((step, idx) => {
            const mark = step.status === 'completed' ? '✓' : step.status === 'in_progress' ? '●' : '○'
            return <li key={`${idx}:${step.content}`} className={`agent-activity__plan-step agent-activity__plan-step--${step.status}`}>
              <span className="agent-activity__plan-mark" aria-hidden="true">{mark}</span>
              <span>{step.content}</span>
            </li>
          })}
        </ul>}
        {run.status === 'running' && running?.recent_tools && running.recent_tools.length > 0 && (() => {
          const tools = running.recent_tools
          const failures = tools.filter(t => t.status !== 'completed').length
          const open = openTools.has(run.run_id)
          return <button type="button" className="agent-activity__tools-toggle" aria-expanded={open} onClick={() => toggleTools(run.run_id)}>
            <span className="agent-activity__tools-caret" aria-hidden="true">▸</span>
            {tools.length} {tools.length === 1 ? 'tool' : 'tools'}
            {failures > 0 && <span className="agent-activity__tools-failed">{failures} failed</span>}
          </button>
        })()}
        {run.status === 'running' && running?.recent_tools && running.recent_tools.length > 0 && openTools.has(run.run_id) && <ul className="agent-activity__tools" aria-label="Completed tool calls">
          {running.recent_tools.map((tool, idx) => {
            const mark = tool.status === 'completed' ? '✓' : '✗'
            return <li key={`${idx}:${tool.finished_at}:${tool.title}`} className={`agent-activity__tool agent-activity__tool--${tool.status}`}>
              <span className="agent-activity__tool-mark" aria-hidden="true">{mark}</span>
              {tool.kind && <span className="agent-activity__tool-kind">{tool.kind}</span>}
              <span className="agent-activity__tool-title">{tool.title}</span>
              <span className="agent-activity__tool-duration">{ago(tool.duration_secs)}</span>
            </li>
          })}
        </ul>}
        {run.detail && run.detail !== label && run.detail !== 'No reply needed' && <p className="agent-activity__detail">{run.detail}</p>}
        {ambient && warden.decided[run.message_id]?.decision && <p className="agent-activity__warden" title={warden.decided[run.message_id].decision!.decider}>
          {warden.decided[run.message_id].phase === 'shadow' ? 'Warden would have chosen: ' : 'Warden: '}
          {describeDecision(warden.decided[run.message_id].decision!)}
        </p>}
        {ambient && warden.votes[run.message_id]?.vote && (() => {
          // What the voters actually did. "N different answers" counted groups
          // of conclusions, so a vote where one agent answered and two failed
          // read the same as three agents disagreeing — and the posted message
          // already carries the verdict, so this only has to add the tally.
          const vote = warden.votes[run.message_id].vote!
          const count = (result: string) => vote.ballots.filter(b => b.result === result).length
          const answered = count('answered')
          const tally = [
            `${answered} of ${vote.ballots.length} answered`,
            count('passed') ? `${count('passed')} passed` : '',
            count('failed') ? `${count('failed')} failed` : '',
          ].filter(Boolean).join(' · ')
          return <p className="agent-activity__warden" title={vote.judged_by ? `Agreement judged by ${vote.judged_by}` : undefined}>
            Blind vote: {tally}
            {vote.posted_by ? ` · posted @${vote.posted_by}` : vote.added ? ` · asked @${vote.added} too` : ''}
          </p>
        })()}
        {local && usage[run.run_id] && (() => {
          const u = usage[run.run_id]
          const ratio = u.context_size > 0 ? Math.min(100, Math.round((u.context_used / u.context_size) * 100)) : null
          const text = describeUsage(u)
          return <>
            {text && <p className="agent-activity__usage" title={usageTitle(u)}>{text}</p>}
            {/* A bar for a window that is mostly empty is noise; it shows
                once the context is worth watching. */}
            {ratio !== null && ratio >= CONTEXT_BAR_FROM && <div className={`agent-activity__context-bar${ratio >= 80 ? ' agent-activity__context-bar--high' : ''}`} role="meter" aria-label="Context window usage" aria-valuemin={0} aria-valuemax={100} aria-valuenow={ratio} title={`Context window ${ratio}% used`}>
              <span style={{ width: `${ratio}%` }} />
            </div>}
          </>
        })()}
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
  const working = active.filter(r => r.status === 'running').length
  const waiting = active.length - working
  const today = spend.length > 0 ? todayCost(spend, new Date()) : undefined
  const section = (label: string) => <h4 className="agent-activity__section"><span>{label}</span></h4>
  return <section aria-label="Agent activity" className="agent-activity">
    <dl className="agent-activity__stats" title="Status is reported by each device and may be out of date while it’s disconnected. A missing entry doesn’t confirm delivery.">
      <div className={working > 0 ? 'is-live' : undefined}><dt>Working</dt><dd>{working}</dd></div>
      <div><dt>Queued</dt><dd>{waiting}</dd></div>
      <div title={spent ? `Today on this device: ${spent}. What this device's agents reported spending since midnight — their figures, not a bill; on a subscription they may be API-price equivalents. Only this device sees it.` : 'Nothing reported today on this device'}>
        <dt>Today</dt><dd>{today ?? '—'}</dd>
      </div>
    </dl>
    {error && <p role="status" className="agent-activity__error">{error}</p>}
    {warnings.map(warning => <p key={warning} className="agent-activity__readiness">{warning}</p>)}
    {/* The only part anyone has to act on, so it comes first and open. */}
    {attention.length > 0 && <details className="agent-activity__group agent-activity__group--attention" open>
      <summary title="Turns from the last day that failed, were interrupted, or never ran, and can be tried again">Failed <span>{attention.length}</span></summary>
      {rows(attention)}
    </details>}
    <div className="agent-activity__block">
      {section('Now')}
      {active.length > 0 ? rows(active) : <p className="agent-activity__empty">Nothing running</p>}
    </div>
    {/* Said once and folded away: each is a fact about an agent, fixed on its
        own device, not a to-do for this one. Members marks them too. */}
    {broken.size > 0 && <details className="agent-activity__group agent-activity__group--broken">
      <summary title="Agents that fail every message on their device until someone fixes them there">Unavailable agents <span>{broken.size}</span></summary>
      <ul className="agent-activity__broken">
        {[...broken.values()].map(b => {
          const host = members.find(m => m.peer_id === b.peerId)
          const device = host?.device_label || (b.peerId === selfPeer ? 'this device' : `device ${b.peerId.slice(-8)}`)
          return <li key={`${b.peerId}|${b.agent}`}>
            <div className="agent-activity__identity">
              <span className="agent-activity__who">
                <AgentIcon name={b.agent} className="agent-activity__bot" />
                <strong className="agent-activity__name">@{b.agent}</strong>
              </span>
              <AgentFixBadge broken={b} device={device} local={b.peerId === selfPeer} />
            </div>
            <div className="agent-activity__meta">
              <span className="agent-activity__device"><Laptop size={11} aria-hidden="true" />{device}</span>
              <span>{b.failures} missed</span>
            </div>
          </li>
        })}
      </ul>
      {onOpenMembers && <button type="button" className="agent-activity__section-link agent-activity__members-link" onClick={onOpenMembers}>See all agents in Members →</button>}
    </details>}
    {history.length > 0 && <details className="agent-activity__group"><summary>Done <span>{history.length}</span></summary>{rows([...history].sort((a, b) => (b.updated_at ?? 0) - (a.updated_at ?? 0)))}</details>}
    {notPickedUp.length > 0 && <details className="agent-activity__group">
      <summary title="Messages this device's agents saw and decided not to answer, with the reason. Cleared when the daemon restarts.">Skipped <span>{notPickedUp.length}</span></summary>
      <ul className="agent-activity__list">
        {notPickedUp.map(skip => {
          const source = messages.find(m => m.id === skip.message_id)
          return <li key={`${skip.message_id}:${skip.agent ?? ''}:${skip.reason}`} className="agent-activity__item agent-activity__item--skipped">
            <div className="agent-activity__identity">
              <strong className="agent-activity__name">{skip.agent ? `@${skip.agent}` : 'Nobody'}</strong>
              <span className="agent-activity__status">Skipped</span>
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
  </section>
}
