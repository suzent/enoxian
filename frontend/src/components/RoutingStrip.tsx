import { useState } from 'react'
import type { ChatMessage, ExecutionRun } from '../types'
import AgentIcon from './AgentIcon'

/**
 * Who a message was routed to, and what happened to it, under the message.
 *
 * Built from the synced execution receipts, so every device tells the same
 * story whichever device's warden or rotation picked the agent. Collapsed to
 * one line (the routing chip) while things are going to plan; opened into the
 * lifecycle strip when a turn failed or the message was handed on, which is
 * when someone wants to know why.
 */

/** Text of a daemon notice this strip replaces, for the message it replies to. */
export function isRoutingNotice(msg: ChatMessage): boolean {
  if (msg.agent_id !== 'system' || !msg.reply_to) return false
  return msg.text.startsWith('No agent could answer') || / could not answer\b/.test(msg.text)
}

export type RoutingState = 'working' | 'answered' | 'passed' | 'failed'

export interface RoutingStep {
  key: string
  tone: 'done' | 'active' | 'failed' | 'muted'
  text: string
  detail?: string
  /** For a step about one agent's turn: who, and how it went, so the strip
   *  can set the name and the outcome apart instead of one run-on sentence. */
  who?: string
  status?: string
  handedOn?: boolean
}

/** Where one agent's turn on the message stands, by its latest run. */
export type AgentOutcome = 'queued' | 'working' | 'answered' | 'voted' | 'passed' | 'failed' | 'stopped'

export interface RoutedAgent {
  name: string
  outcome: AgentOutcome
}

export interface Routing {
  routedBy: 'warden' | 'rotation' | 'mention' | null
  /** The listeners first chosen, as the transcript names them. */
  chosen: string[]
  mode?: string
  confidence?: number
  state: RoutingState
  handedOn: boolean
  steps: RoutingStep[]
  /** Every agent that took a turn on the message, in the order they started. */
  agents: RoutedAgent[]
}

const STATUS_TEXT: Record<ExecutionRun['status'], string> = {
  pending: 'queued',
  running: 'working',
  completed: 'finished',
  failed: 'could not answer',
  interrupted: 'stopped when its daemon restarted',
  expired: 'not run in time',
  cancelled: 'stopped',
  legacy_suppressed: 'outcome unavailable',
}

const TERMINAL_FAILURE = new Set<ExecutionRun['status']>(['failed', 'interrupted', 'expired'])

function pct(p: number) {
  return `${Math.round(Math.max(0, Math.min(1, p)) * 100)}%`
}

/**
 * Fold what the room knows about one message into a routing account.
 *
 * `runs` are every device's receipts for it; `answeredBy` names the agents
 * whose replies to it are in the transcript; `notices` are the daemon's
 * "could not answer" posts for it. Returns `null` when there is nothing worth
 * showing: an addressed turn that went fine needs no explanation.
 */
export function deriveRouting(
  runs: ExecutionRun[],
  answeredBy: string[],
  notices: ChatMessage[],
  nameOf: (agent: string, peer?: string) => string,
): Routing | null {
  const visible = runs.filter(r => r.status !== 'legacy_suppressed')
  if (visible.length === 0 && notices.length === 0) return null
  const ordered = [...visible].sort((a, b) =>
    (a.admitted_at ?? 0) - (b.admitted_at ?? 0) || a.run_id.localeCompare(b.run_id))

  // A run admitted after an earlier one for this message had already failed
  // is the message being handed on: a retry, an untried listener, a voter.
  // Receipts are stamped to the second, so a run admitted in the second the
  // failure landed counts only if it was admitted after the failed run was.
  const failedAt = (r: ExecutionRun) => r.updated_at ?? r.admitted_at ?? 0
  const failing = ordered
    .filter(r => TERMINAL_FAILURE.has(r.status))
    .sort((a, b) => failedAt(a) - failedAt(b))[0]
  const isHandOff = (r: ExecutionRun) => {
    if (!failing || r === failing) return false
    const admitted = r.admitted_at ?? 0
    return admitted > failedAt(failing)
      || (admitted === failedAt(failing) && admitted > (failing.admitted_at ?? 0))
  }
  const first = ordered.filter(r => !isHandOff(r))
  const handedOn = ordered.some(isHandOff)

  const lead = first.find(r => r.routed_by === 'warden') ?? first[0]
  const routedBy = lead?.routed_by ?? (lead ? (lead.ambient ? 'rotation' : 'mention') : null)
  const chosen = [...new Set(first.map(r => nameOf(r.agent_id, r.peer_id)))]

  const answered = answeredBy.length > 0
  const working = visible.some(r => r.status === 'pending' || r.status === 'running')
  const state: RoutingState = answered ? 'answered'
    : working ? 'working'
    : visible.some(r => r.status === 'completed') ? 'passed'
    : 'failed'

  // Nothing to explain: someone named the agent and it answered, or is on it.
  if (routedBy === 'mention' && !handedOn && state !== 'failed' && notices.length === 0) return null
  if (visible.length === 0 && answered) return null

  const steps: RoutingStep[] = []
  const outcomes = new Map<string, AgentOutcome>()
  const note = lead?.warden
  if (routedBy === 'warden' && note) {
    const by = note.decider ? ` (${note.decider}${note.fallback ? ', fell back to rules' : ''})` : ''
    const took = note.latency_ms != null ? ` in ${(note.latency_ms / 1000).toFixed(1)}s` : ''
    steps.push({
      key: 'decided', tone: 'done',
      text: `Warden${by} chose ${note.mode} → ${chosen.join(', ')}${took}`,
      detail: note.reason || undefined,
    })
  } else if (routedBy === 'rotation') {
    steps.push({ key: 'decided', tone: 'done', text: `Rotation picked ${chosen.join(', ')}` })
  } else if (routedBy === 'mention') {
    steps.push({ key: 'decided', tone: 'done', text: `Asked ${chosen.join(', ')} directly` })
  }
  for (const r of ordered) {
    const who = nameOf(r.agent_id, r.peer_id)
    const replied = answeredBy.includes(who) || answeredBy.includes(r.agent_id)
    // In a vote every listener finishes, and only the chosen answer is posted:
    // the others voted rather than passed.
    const voted = (r.warden?.mode ?? note?.mode) === 'vote'
    const status = r.status === 'completed' ? (replied ? 'answered' : voted ? 'voted' : 'passed') : STATUS_TEXT[r.status]
    // Later runs for the same agent overwrite earlier ones: a retry that
    // answered means the agent answered.
    outcomes.set(who, replied && r.status === 'completed' ? 'answered'
      : r.status === 'completed' ? (voted ? 'voted' : 'passed')
      : r.status === 'running' ? 'working'
      : r.status === 'pending' ? 'queued'
      : TERMINAL_FAILURE.has(r.status) ? 'failed'
      : 'stopped')
    steps.push({
      who, status, handedOn: isHandOff(r),
      key: r.run_id,
      tone: r.status === 'pending' || r.status === 'running' ? 'active'
        : TERMINAL_FAILURE.has(r.status) ? 'failed'
        : r.status === 'completed' ? 'done' : 'muted',
      text: `${isHandOff(r) ? 'Handed on to ' : ''}${who}: ${status}`,
      // A stopped run says why, too: "stopped" alone reads as a dead end.
      detail: (TERMINAL_FAILURE.has(r.status) || r.status === 'cancelled') && r.detail ? r.detail : undefined,
    })
  }
  if (answered) {
    steps.push({ key: 'answered', tone: 'done', text: `Answered by ${answeredBy.join(', ')}` })
  } else if (!working) {
    // The daemon's own verdict, from whichever devices posted one. Shown here
    // rather than as a separate notice, and dropped once anyone answers.
    for (const n of notices) steps.push({ key: n.id, tone: 'failed', text: n.text })
    if (notices.length === 0 && state === 'failed') {
      steps.push({ key: 'unanswered', tone: 'failed', text: 'No answer yet' })
    }
  }

  return {
    routedBy,
    chosen,
    mode: note?.mode,
    confidence: chosen.length === 1 ? note?.confidence : undefined,
    state,
    handedOn,
    steps,
    agents: [...outcomes].map(([name, outcome]) => ({ name, outcome })),
  }
}

const STATE_TEXT: Record<RoutingState, string> = {
  working: 'working',
  answered: 'answered',
  passed: 'no reply',
  failed: 'no answer',
}

const OUTCOME_TEXT: Record<AgentOutcome, string> = {
  queued: 'queued',
  working: 'working',
  answered: 'answered',
  voted: 'voted',
  passed: 'passed',
  failed: 'failed',
  stopped: 'stopped',
}

const MODE_TEXT: Record<string, string> = {
  vote: 'vote',
  together: 'together',
}

export default function RoutingStrip({ routing }: { routing: Routing }) {
  const opensItself = routing.state === 'failed' || (routing.handedOn && routing.state !== 'answered')
  const [open, setOpen] = useState<boolean | null>(null)
  const expanded = open ?? opensItself
  const who = routing.routedBy === 'warden' ? 'Warden'
    : routing.routedBy === 'rotation' ? 'Rotation'
    : routing.routedBy === 'mention' ? 'Asked'
    : 'Agents'
  // Agents that only joined after a failure are named in the steps, not here:
  // the chip says who was chosen, and the dots say how each did.
  const outcomeOf = new Map(routing.agents.map(a => [a.name, a.outcome]))
  const meta = [
    routing.mode && routing.mode !== 'single' ? (MODE_TEXT[routing.mode] ?? routing.mode) : null,
    routing.confidence != null ? pct(routing.confidence) : null,
    routing.handedOn ? 'handed on' : null,
  ].filter(Boolean)
  return (
    <div className={`chat-routing chat-routing--${routing.state}`} role="group" aria-label="How this message was routed">
      <button
        type="button"
        className="chat-routing__chip"
        aria-expanded={expanded}
        onClick={() => setOpen(!expanded)}
        title={expanded ? 'Hide how this message was routed' : 'Show how this message was routed'}
      >
        <span className="chat-routing__caret" aria-hidden="true">▸</span>
        <span className="chat-routing__who">{who}</span>
        {routing.chosen.map(name => {
          const outcome = outcomeOf.get(name)
          return (
            <span key={name} className={`chat-routing__agent chat-routing__agent--${outcome ?? 'queued'}`} title={outcome ? `${name}: ${OUTCOME_TEXT[outcome]}` : name}>
              <span className="chat-routing__dot-inline" aria-hidden="true" />
              <AgentIcon name={name} size={11} className="chat-routing__agent-icon" />
              {name}
            </span>
          )
        })}
        {meta.map(m => <span key={m} className="chat-routing__meta">· {m}</span>)}
        <span className={`chat-routing__state chat-routing__state--${routing.state}`}>{STATE_TEXT[routing.state]}</span>
      </button>
      {expanded && (
        <ol className="chat-routing__steps">
          {routing.steps.map(step => (
            <li key={step.key} className={`chat-routing__step chat-routing__step--${step.tone}`}>
              <span className="chat-routing__dot" aria-hidden="true" />
              {step.who ? (
                <span className="chat-routing__text chat-routing__text--agent">
                  {step.handedOn && <span className="chat-routing__handoff">handed on to</span>}
                  <span className="chat-routing__name">{step.who}</span>
                  <span className={`chat-routing__outcome chat-routing__outcome--${step.tone}`}>{step.status}</span>
                </span>
              ) : (
                <span className="chat-routing__text">{step.text}</span>
              )}
              {step.detail && <span className="chat-routing__detail">{step.detail}</span>}
            </li>
          ))}
        </ol>
      )}
    </div>
  )
}
