import Select from './ui/Select'
import { useCallback, useEffect, useRef, useState } from 'react'
import { cancelAgentAuth, getAgentAuth, startAgentAuth, type AgentAuthState } from '../api'

export function authActive(state?: AgentAuthState): boolean {
  return !!state && ['checking', 'authenticating', 'terminal', 'verifying'].includes(state.status)
}

/** Statuses worth checking again when the operator comes back: they may have signed in elsewhere. */
const RECHECK_ON_RETURN: AgentAuthState['status'][] = ['auth_required', 'failed', 'cancelled']
/** A signed-in agent is checked again on return only after this long, since a check starts the agent. */
const READY_RECHECK_MS = 5 * 60_000

export function useAgentAuthentication(enabled: boolean, circleId?: string | null) {
  const [states, setStates] = useState<Record<string, AgentAuthState>>({})
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [pending, setPending] = useState<Record<string, boolean>>({})
  const statesRef = useRef(states)
  statesRef.current = states
  const autoChecked = useRef(new Set<string>())
  useEffect(() => {
    if (!enabled) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout>
    const poll = async () => {
      try {
        const next = await getAgentAuth()
        if (!cancelled) { setStates(next); setLoaded(true); setError(null) }
      } catch {
        if (!cancelled) setError('Could not read sign-in status. Check the local daemon connection.')
      } finally { if (!cancelled) timer = setTimeout(() => void poll(), 1000) }
    }
    void poll()
    return () => { cancelled = true; clearTimeout(timer) }
  }, [enabled])
  const act = useCallback(async (id: string, action: 'probe' | 'login' | 'cancel', method?: string) => {
    setPending(previous => ({ ...previous, [id]: true }))
    try {
      if (action === 'cancel') await cancelAgentAuth(id)
      else {
        const state = await startAgentAuth(id, action, method, circleId)
        setStates(previous => ({ ...previous, [id]: state }))
      }
      setError(null)
    } catch {
      setError('Could not start or cancel sign-in. Check the agent configuration and retry.')
    } finally { setPending(previous => ({ ...previous, [id]: false })) }
  }, [circleId])
  // A check the operator did not ask for stays quiet when it cannot start
  // (the daemon limits concurrent checks); the next return to the window retries.
  const probedAt = useRef(new Map<string, number>())
  const probeQuietly = useCallback(async (id: string) => {
    probedAt.current.set(id, Date.now())
    try {
      const state = await startAgentAuth(id, 'probe', undefined, circleId)
      if (state) setStates(previous => ({ ...previous, [id]: state }))
    } catch { /* the next poll or return to the window covers it */ }
  }, [circleId])
  /** Checks an agent once, as soon as the daemon has said it has no result for it yet. */
  const autoCheck = useCallback((id: string) => {
    if (!enabled || !loaded || autoChecked.current.has(id)) return
    const status = statesRef.current[id]?.status ?? 'unknown'
    if (status !== 'unknown') return
    autoChecked.current.add(id)
    void probeQuietly(id)
  }, [enabled, loaded, probeQuietly])
  useEffect(() => {
    if (!enabled) return
    const recheck = () => {
      if (document.visibilityState !== 'visible') return
      for (const [id, state] of Object.entries(statesRef.current)) {
        // A result this page did not ask for counts as fresh from first sight.
        if (!probedAt.current.has(id)) probedAt.current.set(id, Date.now())
        const stale = Date.now() - probedAt.current.get(id)! > READY_RECHECK_MS
        if (RECHECK_ON_RETURN.includes(state.status) || (state.status === 'ready' && stale)) void probeQuietly(id)
      }
    }
    window.addEventListener('focus', recheck)
    document.addEventListener('visibilitychange', recheck)
    return () => { window.removeEventListener('focus', recheck); document.removeEventListener('visibilitychange', recheck) }
  }, [enabled, probeQuietly])
  return { states, error, pending, act, autoCheck }
}

// Short enough for one line beside the controls; the in-progress ones say
// where to finish, since nothing else on the row does.
const LABELS: Record<AgentAuthState['status'], string> = {
  unknown: 'Not checked', checking: 'Checking…', auth_required: 'Not signed in',
  authenticating: 'Finish in the browser…', terminal: 'Finish in the terminal…',
  verifying: 'Verifying…', ready: 'Signed in', failed: 'Check failed', cancelled: 'Sign-in cancelled',
}
const PRIMARY_LABELS = ['SIGN IN', 'SIGN IN AGAIN', 'CANCEL'] as const

interface Props {
  agent: string
  state?: AgentAuthState
  pending?: boolean
  onAction: (agent: string, action: 'probe' | 'login' | 'cancel', method?: string) => Promise<void>
  /** Called while no check has run for this agent; the hook decides whether one starts. */
  onAutoCheck?: (agent: string) => void
}
export default function AgentAuthentication({ agent, state, pending, onAction, onAutoCheck }: Props) {
  const [method, setMethod] = useState('')
  const status = state?.status ?? 'unknown'
  const active = authActive(state)
  // While a check or sign-in runs, keep showing the controls of the state it
  // started from, so pressing a button never adds or removes one.
  const idleSignedIn = useRef(false)
  if (!active) idleSignedIn.current = status === 'ready'
  const signedIn = idleSignedIn.current
  // A new check starts with no methods until the agent reports them again;
  // keep the last list so the method picker does not vanish mid-check.
  const lastMethods = useRef<AgentAuthState['methods']>([])
  if (state?.methods.length) lastMethods.current = state.methods
  const methods = lastMethods.current.filter(item => item.available)
  useEffect(() => {
    if (status === 'unknown') onAutoCheck?.(agent)
  }, [agent, status, onAutoCheck])
  // One button slot: SIGN IN / SIGN IN AGAIN while idle, CANCEL while a
  // check or sign-in runs; every label sits in the same cell, so the button
  // is always as wide as the longest.
  const primary = active ? 'CANCEL' : signedIn ? 'SIGN IN AGAIN' : 'SIGN IN'
  const onPrimary = () => active
    ? void onAction(agent, 'cancel')
    : void onAction(agent, 'login', methods.some(item => item.id === method) ? method : undefined)
  const tone = status === 'ready' ? 'is-ready' : active ? 'is-busy' : 'is-idle'
  return (
    // Status, controls and error are siblings so a host layout (the agent
    // card in settings) can place them into its own grid.
    <div className="agent-auth">
      <span className={`agent-auth__status ${tone}`} role="status" title={state?.error ?? LABELS[status]}>
        <i aria-hidden="true" />{LABELS[status]}
      </span>
      <div className="agent-auth__controls">
        {!signedIn && methods.length > 1 && <Select aria-label={`@${agent} sign-in method`} value={method} disabled={active || !!pending}
          onChange={setMethod} options={[{ value: '', label: 'Automatic' }, ...methods.map(item => ({ value: item.id, label: item.name }))]} />}
        <button type="button" className="agent-btn agent-auth__primary" aria-label={primary} disabled={!!pending}
          onClick={onPrimary}>
          {PRIMARY_LABELS.map(label => (
            <span key={label} aria-hidden="true" className={label === primary ? '' : 'invisible'}>{label}</span>
          ))}
        </button>
      </div>
      {state?.error && <p role="alert" className="agent-auth__error">{state.error}</p>}
    </div>
  )
}
