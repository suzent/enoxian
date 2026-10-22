import { useCallback, useEffect, useState } from 'react'
import { RefreshCw } from 'lucide-react'
import { cancelAgentAuth, getAgentAuth, startAgentAuth, type AgentAuthState } from '../api'

export function authActive(state?: AgentAuthState): boolean {
  return !!state && ['checking', 'authenticating', 'terminal', 'verifying'].includes(state.status)
}

export function useAgentAuthentication(enabled: boolean) {
  const [states, setStates] = useState<Record<string, AgentAuthState>>({})
  const [error, setError] = useState<string | null>(null)
  const [pending, setPending] = useState<Record<string, boolean>>({})
  useEffect(() => {
    if (!enabled) return
    let cancelled = false
    let timer: ReturnType<typeof setTimeout>
    const poll = async () => {
      try {
        const next = await getAgentAuth()
        if (!cancelled) { setStates(next); setError(null) }
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
        const state = await startAgentAuth(id, action, method)
        setStates(previous => ({ ...previous, [id]: state }))
      }
      setError(null)
    } catch {
      setError('Could not start or cancel sign-in. Check the agent configuration and retry.')
    } finally { setPending(previous => ({ ...previous, [id]: false })) }
  }, [])
  return { states, error, pending, act }
}

const LABELS: Record<AgentAuthState['status'], string> = {
  unknown: 'Sign-in not checked', checking: 'Checking sign-in…', auth_required: 'Sign-in required',
  authenticating: 'Complete sign-in in the browser or agent window.', terminal: 'Complete sign-in in the terminal opened on this device.',
  verifying: 'Verifying sign-in…', ready: 'Signed in', failed: 'Sign-in could not be verified', cancelled: 'Sign-in cancelled',
}
interface Props {
  agent: string
  state?: AgentAuthState
  pending?: boolean
  onAction: (agent: string, action: 'probe' | 'login' | 'cancel', method?: string) => Promise<void>
}
export default function AgentAuthentication({ agent, state, pending, onAction }: Props) {
  const [method, setMethod] = useState('')
  const active = authActive(state)
  const busy = !!pending || active
  const signedIn = state?.status === 'ready'
  const methods = state?.methods.filter(item => item.available) ?? []
  return (
    <div className="mt-2 border border-obsidian/30 bg-alabaster p-2 text-obsidian space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[10px] font-bold" role="status">{LABELS[state?.status ?? 'unknown']}</span>
        <button type="button" className="enox-btn p-1 min-h-0 ml-auto" title={`Check @${agent} sign-in`} aria-label={`Check @${agent} sign-in`}
          disabled={busy} onClick={() => void onAction(agent, 'probe')}>
          <RefreshCw size={13} aria-hidden="true" className={state?.status === 'checking' ? 'animate-spin' : ''} />
        </button>
        {!signedIn && methods.length > 1 && <select aria-label={`@${agent} sign-in method`} value={method} disabled={busy}
          onChange={event => setMethod(event.target.value)} className="border border-obsidian bg-alabaster text-obsidian text-[10px] p-1 max-w-full">
          <option value="">Automatic</option>
          {methods.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
        </select>}
        <button type="button" disabled={busy} className="enox-btn text-[10px] px-2 py-1 min-h-0"
          onClick={() => void onAction(agent, 'login', methods.some(item => item.id === method) ? method : undefined)}>
          {signedIn ? 'SIGN IN AGAIN' : 'SIGN IN'}
        </button>
        {active && <button type="button" disabled={pending} className="enox-btn text-[10px] px-2 py-1 min-h-0"
          onClick={() => void onAction(agent, 'cancel')}>CANCEL</button>}
      </div>
      {state?.error && <p role="alert" className="text-[10px]">{state.error}</p>}
      {state?.status === 'unknown' && <p className="text-[9px]">Installation and sign-in are checked separately.</p>}
    </div>
  )
}
