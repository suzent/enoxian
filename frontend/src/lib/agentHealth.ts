import type { ExecutionRun } from '../types'

/**
 * Which agents are broken where, read from the synced run receipts.
 *
 * A failure that is the agent's rather than the message's — an expired
 * sign-in, a program that will not start — repeats on every message until
 * someone fixes it on that device. The running device already leaves such an
 * agent out of the room (`src/agent/health.rs`); this is how every other
 * device finds out, with nothing new synced: the agent's latest finished turn
 * on that device says whether it is still broken.
 *
 * `classifyFailure` mirrors `classify_text` in `src/agent/health.rs`; keep the
 * two in step. The {@link FAILING_STREAK} fallback is shown here only: the
 * daemon leaves an agent out just for failures it can name.
 */

export type AgentProblem = 'sign_in' | 'missing' | 'limited' | 'failing'

const SIGN_IN = [
  'failed to authenticate',
  'authentication required',
  'authorization required',
  'not logged in',
  'please log in',
  'please run /login',
  'invalid api key',
  'oauth token has expired',
  'oauth session expired',
  '(code -32000)',
]

const LIMITED = [
  'hit your session limit',
  'hit your usage limit',
  'usage limit reached',
  'quota exceeded',
  'exceeded your current quota',
]

/** Turns in a row that fail, whatever the reason, before an agent counts as
 *  broken on its device. Catches what no list of messages can: a bug in an
 *  older daemon, a misconfigured adapter, anything that fails every time. */
export const FAILING_STREAK = 3

export function classifyFailure(detail?: string | null): AgentProblem | null {
  if (!detail) return null
  const text = detail.toLowerCase()
  // A session this authorization can no longer reach is not a sign-in
  // problem: a fresh session fixes it.
  if (text.includes('cannot access') && text.includes('session')) return null
  if (SIGN_IN.some(needle => text.includes(needle))) return 'sign_in'
  if (text.includes('failed to spawn')) return 'missing'
  if (LIMITED.some(needle => text.includes(needle))) return 'limited'
  return null
}

export interface BrokenAgent {
  agent: string
  peerId: string
  problem: AgentProblem
  /** The latest failure's own words. */
  detail: string
  /** Failed turns since its last success. */
  failures: number
  since: number
}

/** A word or two, for a chip. */
export const PROBLEM_TEXT: Record<AgentProblem, string> = {
  sign_in: 'sign-in',
  missing: "won't start",
  limited: 'limit',
  failing: 'failing',
}

/** The same as a sentence, for a tooltip. */
export const PROBLEM_LONG: Record<AgentProblem, string> = {
  sign_in: 'needs to sign in again',
  missing: "won't start",
  limited: 'has hit its usage limit',
  failing: 'keeps failing',
}

export function agentName(run: Pick<ExecutionRun, 'agent_id'>): string {
  return run.agent_id.replace(/^~ambient:/, '')
}

export function healthKey(peerId: string | undefined | null, agent: string): string {
  return `${peerId ?? ''}|${agent.toLowerCase()}`
}

const FINISHED = new Set<ExecutionRun['status']>(['completed', 'failed'])

/** Agents whose latest finished turn on their device failed in a way only
 *  fixing the agent cures, keyed by {@link healthKey}. */
export function brokenAgents(runs: ExecutionRun[]): Map<string, BrokenAgent> {
  const byAgent = new Map<string, ExecutionRun[]>()
  for (const run of runs) {
    if (!FINISHED.has(run.status) || !run.peer_id) continue
    const key = healthKey(run.peer_id, agentName(run))
    byAgent.set(key, [...(byAgent.get(key) ?? []), run])
  }
  const broken = new Map<string, BrokenAgent>()
  for (const [key, list] of byAgent) {
    list.sort((a, b) => (b.updated_at ?? 0) - (a.updated_at ?? 0))
    const latest = list[0]
    if (latest.status !== 'failed') continue
    const streak = list.findIndex(r => r.status === 'completed')
    const failing = streak === -1 ? list : list.slice(0, streak)
    const problem = classifyFailure(latest.detail) ?? (failing.length >= FAILING_STREAK ? 'failing' : null)
    if (!problem) continue
    broken.set(key, {
      agent: agentName(latest),
      peerId: latest.peer_id!,
      problem,
      detail: latest.detail ?? '',
      failures: failing.length,
      since: failing[failing.length - 1].updated_at ?? latest.updated_at ?? 0,
    })
  }
  return broken
}
