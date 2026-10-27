/**
 * Offer this device the recommended agent behaviour, with exactly what would
 * change, and remember the answer either way.
 *
 * Settings are written back whole on every save, so a device keeps the
 * defaults of whatever version first saved them. This is how a better default
 * reaches it without overriding a choice someone made: asked once per
 * version, and always available again under Agent behaviour.
 */
import { useEffect, useState } from 'react'
import { getAgentConfig, getWarden, setEngagement, type WardenView } from '../api'
import type { AgentConfigView, SettingsView } from '../types'
import Dialog from './ui/Dialog'

/** Bump when the recommendation changes, to ask every device again. */
export const RECOMMENDED_VERSION = 1

/** Follow-ups without a mention, for three minutes after an agent replies. */
export const RECOMMENDED_WINDOW_SECS = 180

/** Wide enough that the Warden's spec, not this ceiling, decides. */
export const OPEN_CEILING = 32

type Patch = Parameters<typeof setEngagement>[0]

export interface Row {
  label: string
  from: string
  to: string
  /** Shown instead of applying, when something else has to happen first. */
  blocked?: boolean
}

/** Whether the chosen decider can actually decide, rather than fall back to the rules. */
export function wardenReady(warden: WardenView | null) {
  if (!warden) return false
  const kind = warden.kinds.find(k => k.id === warden.config.decider)
  if (!kind || kind.protocol === 'rules') return false
  return !kind.needs_key || !!kind.key_hint || !!warden.config.key_env
}

const reaction = (r: string) => (r === 'push' ? 'Starts right away' : 'Waits until fetched')
const window_ = (s: number) => (s > 0 ? `${s}s` : 'Off')
const names = (list: string[]) => (list.length ? list.join(', ') : 'None')
const mode = (m?: string) => ({ act: 'Decides who answers', shadow: 'Records only' }[m ?? ''] ?? 'Off')

/** The patch and the rows that explain it, for this device as it is now. */
export function recommendation(cfg: AgentConfigView, warden: WardenView | null) {
  const now: SettingsView = cfg.global_settings
  const agents = cfg.agents.filter(a => a.installed).map(a => a.name)
  const ready = wardenReady(warden)
  const patch: Patch = { reaction: 'push', engagement_window_secs: RECOMMENDED_WINDOW_SECS }
  const rows: Row[] = []

  if (now.reaction !== 'push') rows.push({ label: 'When an agent is @mentioned', from: reaction(now.reaction), to: reaction('push') })
  if (now.engagement_window_secs !== RECOMMENDED_WINDOW_SECS) {
    rows.push({ label: 'Follow-ups without a mention', from: window_(now.engagement_window_secs), to: window_(RECOMMENDED_WINDOW_SECS) })
  }
  if (agents.length && agents.some(a => !now.ambient.includes(a))) {
    const next = [...new Set([...now.ambient, ...agents])]
    patch.ambient = next
    rows.push({ label: 'Agents reading the room', from: names(now.ambient), to: names(next) })
  }
  if (ready) {
    if (now.warden_mode !== 'act') {
      patch.warden_mode = 'act'
      rows.push({ label: 'Warden', from: mode(now.warden_mode), to: mode('act') })
    }
    if ((now.ambient_responders ?? 1) < OPEN_CEILING) {
      patch.ambient_responders = OPEN_CEILING
      rows.push({ label: 'Agents one message may reach', from: String(now.ambient_responders ?? 1), to: `Up to ${OPEN_CEILING}` })
    }
  } else if (now.warden_mode !== 'act') {
    rows.push({ label: 'Warden', from: mode(now.warden_mode), to: 'Save a key under Settings → Warden first', blocked: true })
  }
  return { patch, rows }
}

interface Props {
  /** Ask only if this device has not answered and something would change. */
  auto?: boolean
  onClose: (applied: boolean) => void
}

export default function RecommendedSetup({ auto, onClose }: Props) {
  const [state, setState] = useState<{ cfg: AgentConfigView; warden: WardenView | null } | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let live = true
    Promise.all([getAgentConfig(), getWarden().catch(() => null)]).then(([cfg, warden]) => {
      if (live) setState({ cfg, warden })
    }, e => {
      if (!live) return
      if (auto) onClose(false)
      else setError(e instanceof Error ? e.message : String(e))
    })
    return () => { live = false }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const plan = state && recommendation(state.cfg, state.warden)
  const changes = plan?.rows.filter(r => !r.blocked) ?? []
  const asked = (state?.cfg.recommended_seen ?? 0) >= RECOMMENDED_VERSION
  const skip = auto && !!state && (asked || changes.length === 0)

  useEffect(() => { if (skip) onClose(false) }, [skip, onClose])
  if (auto && (!state || skip)) return null

  const answer = async (apply: boolean) => {
    if (!plan) return
    setBusy(true)
    setError(null)
    try {
      await setEngagement({ ...(apply ? plan.patch : {}), recommended_seen: RECOMMENDED_VERSION })
      onClose(apply)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  return (
    <Dialog title="Recommended settings" onClose={() => (auto ? void answer(false) : onClose(false))} className="settings-confirm recommended-setup">
      <div className="ritual-panel__body">
        <p className="recommended-setup__lead">
          Agents answer when you mention them, keep a conversation going, and the Warden picks who takes
          an unaddressed message. Only this device’s global settings change; Circles that set their own keep them.
        </p>
        {!plan ? (
          <p className="recommended-setup__lead" role={error ? 'alert' : 'status'}>{error ?? 'Loading…'}</p>
        ) : plan.rows.length === 0 ? (
          <p className="recommended-setup__lead">This device already uses the recommended settings.</p>
        ) : (
          <table className="recommended-setup__table">
            <thead><tr><th scope="col">Setting</th><th scope="col">Now</th><th scope="col">Recommended</th></tr></thead>
            <tbody>
              {plan.rows.map(row => (
                <tr key={row.label} className={row.blocked ? 'is-blocked' : undefined}>
                  <th scope="row">{row.label}</th><td>{row.from}</td><td>{row.to}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <p className="recommended-setup__feedback" role={error && plan ? 'alert' : undefined}>
          {plan && error ? error : 'You can change any of these later under Agent behaviour.'}
        </p>
        <div className="ritual-actions">
          <button type="button" className="ritual-btn ritual-btn--secondary" disabled={busy || !plan} onClick={() => void answer(false)}>
            KEEP AS IS
          </button>
          <button type="button" className="ritual-btn ritual-btn--primary" disabled={busy || changes.length === 0} onClick={() => void answer(true)}>
            USE RECOMMENDED
          </button>
        </div>
      </div>
    </Dialog>
  )
}
