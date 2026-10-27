/**
 * The two steps after a first Circle is created or joined: connect the agents
 * found on this machine, then choose how the Warden decides who answers.
 *
 * Agents are detected; keys never are. A key reaches the daemon only when the
 * person pastes it here.
 */
import { useCallback, useEffect, useState } from 'react'
import { Check } from 'lucide-react'
import {
  addAgent, discoverAgents, getAgentConfig, getAgentPlugins, getWarden, installAgentPlugin,
  setEngagement, setWardenConfig, setWardenKey, testWarden,
  type WardenDecision, type WardenView,
} from '../api'
import type { AgentPlugin, DiscoveredAgent } from '../types'
import AgentAuthentication, { useAgentAuthentication } from './AgentAuthentication'
import DeciderPicker, { RECOMMENDED_DECIDER, WARDEN_SAMPLES } from './DeciderPicker'
import { describeDecision } from './WardenSettings'
import { OPEN_CEILING, RECOMMENDED_VERSION, RECOMMENDED_WINDOW_SECS } from './RecommendedSetup'

const message = (e: unknown) => (e instanceof Error ? e.message : String(e))
const capitalize = (s: string) => s.charAt(0).toUpperCase() + s.slice(1)

interface Props {
  circleId?: string | null
  onDone: () => void
}

export default function OnboardingSetup({ circleId, onDone }: Props) {
  const [step, setStep] = useState<'agents' | 'warden'>('agents')
  const [agents, setAgents] = useState<string[]>([])
  // Skipping is an answer too: do not ask again about the recommended settings
  // the moment the Circle opens. They stay one click away under Agent behaviour.
  const skip = () => {
    void setEngagement({ recommended_seen: RECOMMENDED_VERSION }).catch(() => {}).finally(onDone)
  }
  return (
    <div className="onboard">
      <div className="onboard__head">
        <span className="onboard__step">STEP {step === 'agents' ? 1 : 2} OF 2</span>
        <h2>{step === 'agents' ? 'YOUR AGENTS' : 'WHO ANSWERS'}</h2>
      </div>
      {step === 'agents'
        ? <AgentsStep circleId={circleId} onNext={names => { setAgents(names); setStep('warden') }} onSkip={skip} />
        : <WardenStep circleId={circleId} agents={agents} onBack={() => setStep('agents')} onSkip={skip} onDone={onDone} />}
    </div>
  )
}

function pluginStatus(p: AgentPlugin) {
  const runtimeMissing = p.runtime_installed === false
  const nodeMissing = p.requires_node && !p.node_runtime_installed
  const ready = p.state === 'ready' && p.configured && !runtimeMissing && !nodeMissing
  return { runtimeMissing, nodeMissing, ready }
}

function AgentsStep({ circleId, onNext, onSkip }: { circleId?: string | null; onNext: (agents: string[]) => void; onSkip: () => void }) {
  const auth = useAgentAuthentication(true, circleId)
  const [plugins, setPlugins] = useState<AgentPlugin[] | null>(null)
  const [extra, setExtra] = useState<DiscoveredAgent[]>([])
  const [configured, setConfigured] = useState<string[]>([])
  const [working, setWorking] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    try {
      const [p, d, cfg] = await Promise.all([getAgentPlugins(), discoverAgents(), getAgentConfig()])
      const managed = new Set(p.plugins.map(x => x.agent))
      setPlugins(p.plugins)
      setExtra(d.agents.filter(a => a.installed && !managed.has(a.name)))
      setConfigured(cfg.agents.map(a => a.name))
      setError(null)
    } catch (e) { setError(message(e)); setPlugins(p => p ?? []) }
  }, [])
  useEffect(() => { void refresh() }, [refresh])

  const act = async (id: string, fn: () => Promise<unknown>) => {
    setWorking(id)
    setError(null)
    try { await fn(); await refresh() } catch (e) { setError(message(e)) } finally { setWorking(null) }
  }

  if (!plugins) return <div className="onboard__body"><p className="onboard__muted" role="status">Looking for agents on this machine…</p></div>

  // Ready first, then what one click sets up, then what needs installing.
  const rank = (p: AgentPlugin) => {
    const s = pluginStatus(p)
    return s.ready ? 0 : s.runtimeMissing || s.nodeMissing ? 2 : 1
  }
  const ordered = [...plugins].sort((a, b) => rank(a) - rank(b))
  const readyCount = configured.length

  return (
    <div className="onboard__body">
      <p className="onboard__lead">
        Agents on this machine can answer in your Circles. Enable the ones you use; their logins and settings stay yours.
      </p>
      {error && <p className="onboard__error" role="alert">{error}</p>}
      <ul className="onboard-agents">
        {ordered.map(p => {
          const s = pluginStatus(p)
          const native = p.kind === 'native'
          const busy = working === p.id || p.state === 'installing'
          return (
            <li key={p.id} className={`onboard-agent${s.ready ? ' is-ready' : ''}`}>
              <div className="onboard-agent__row">
                <div className="onboard-agent__name">
                  <strong>@{p.agent}</strong>
                  <span>
                    {s.ready ? 'Enabled'
                      : s.runtimeMissing ? `${p.runtime_program || 'CLI'} not found`
                        : s.nodeMissing ? 'Needs Node.js 22+'
                          : busy ? (native ? 'Enabling…' : 'Installing adapter…')
                            : p.state === 'ready' || native ? 'Found · not enabled' : 'Found · adapter needed'}
                  </span>
                </div>
                {s.ready ? (
                  <span className="onboard-agent__ok"><Check size={14} aria-hidden="true" /> READY</span>
                ) : s.runtimeMissing ? (
                  p.install_url
                    ? <a className="onboard-btn onboard-btn--ghost" href={p.install_url} target="_blank" rel="noreferrer">GET IT ↗</a>
                    : <span className="onboard__muted">NOT INSTALLED</span>
                ) : s.nodeMissing ? (
                  <a className="onboard-btn onboard-btn--ghost" href="https://nodejs.org/en/download" target="_blank" rel="noreferrer">NODE.JS ↗</a>
                ) : (
                  <button type="button" className="onboard-btn" disabled={!!working || busy}
                    onClick={() => void act(p.id, () => installAgentPlugin(p.id))}>
                    {native || p.state === 'ready' ? 'ENABLE' : 'INSTALL'}
                  </button>
                )}
              </div>
              {s.ready && <AgentAuthentication agent={p.agent} state={auth.states[p.agent]} pending={auth.pending[p.agent]} onAction={auth.act} />}
            </li>
          )
        })}
        {extra.map(a => (
          <li key={a.name} className={`onboard-agent${a.configured ? ' is-ready' : ''}`}>
            <div className="onboard-agent__row">
              <div className="onboard-agent__name">
                <strong>@{a.name}</strong>
                <span>{a.configured ? 'Enabled' : a.about || 'Found on this machine'}</span>
              </div>
              {a.configured
                ? <span className="onboard-agent__ok"><Check size={14} aria-hidden="true" /> READY</span>
                : <button type="button" className="onboard-btn" disabled={!!working}
                    onClick={() => void act(a.name, () => addAgent(a.name, a.driver, a.command))}>ENABLE</button>}
            </div>
          </li>
        ))}
        {ordered.length === 0 && extra.length === 0 && (
          <li className="onboard__muted">No supported agents were found. You can add one later in Settings → Your agents.</li>
        )}
      </ul>
      {auth.error && <p className="onboard__muted" role="alert">{auth.error}</p>}
      {plugins.some(p => pluginStatus(p).runtimeMissing || pluginStatus(p).nodeMissing) && (
        <button type="button" className="onboard-link" onClick={() => void refresh()} disabled={!!working}>Installed something? Check again</button>
      )}
      <div className="onboard__actions">
        <button type="button" className="onboard-btn onboard-btn--primary" disabled={!!working} onClick={() => onNext(configured)}>
          {readyCount ? `CONTINUE WITH ${readyCount} AGENT${readyCount === 1 ? '' : 'S'}` : 'CONTINUE'}
        </button>
        <button type="button" className="onboard-btn onboard-btn--ghost" disabled={!!working} onClick={onSkip}>SKIP SETUP</button>
      </div>
    </div>
  )
}

function WardenStep({ circleId, agents, onBack, onSkip, onDone }: {
  circleId?: string | null; agents: string[]; onBack: () => void; onSkip: () => void; onDone: () => void
}) {
  const [view, setView] = useState<WardenView | null>(null)
  const [choice, setChoice] = useState<string>(RECOMMENDED_DECIDER)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [asking, setAsking] = useState(false)
  const [tried, setTried] = useState<{ text: string; decision: WardenDecision } | null>(null)

  useEffect(() => {
    getWarden().then(v => {
      setView(v)
      // Someone who already chose a decider keeps it; a new device starts on
      // the recommended one, which is only used once it has a key.
      if (v.config.decider !== 'builtin:rules' || v.kinds.every(k => k.id !== RECOMMENDED_DECIDER)) setChoice(v.config.decider)
    }).catch(e => setError(message(e)))
  }, [])

  const reload = async () => setView(await getWarden())

  const guard = async (fn: () => Promise<void>) => {
    setBusy(true)
    setError(null)
    try { await fn() } catch (e) { setError(message(e)) } finally { setBusy(false) }
  }

  const tryIt = (text: string) => guard(async () => {
    setAsking(true)
    try {
      await setWardenConfig({ decider: choice })
      const result = await testWarden(text, circleId)
      setTried({ text, decision: result.decision })
    } finally { setAsking(false) }
  })

  const finish = () => guard(async () => {
    await setWardenConfig({ decider: choice })
    // Open by default: agents hear the room, and the Warden picks who answers.
    await setEngagement({
      reaction: 'push',
      engagement_window_secs: RECOMMENDED_WINDOW_SECS,
      warden_mode: 'act',
      ambient_responders: OPEN_CEILING,
      ...(agents.length ? { ambient: agents } : {}),
      recommended_seen: RECOMMENDED_VERSION,
    })
    onDone()
  })

  if (!view) {
    return (
      <div className="onboard__body">
        {error ? <p className="onboard__error" role="alert">{error}</p> : <p className="onboard__muted" role="status">Loading…</p>}
        <div className="onboard__actions">
          <button type="button" className="onboard-btn onboard-btn--ghost" onClick={onSkip}>SKIP</button>
        </div>
      </div>
    )
  }

  return (
    <div className="onboard__body">
      <p className="onboard__lead">
        When a message names no one, the Warden decides whether it needs an answer and which
        {agents.length ? ` of your ${agents.length === 1 ? 'agent' : `${agents.length} agents`}` : ' agents'} should give it. Mention someone with @ to skip it.
      </p>
      {agents.length === 0 && (
        <p className="onboard__muted">No agents are enabled yet, so there is nobody for it to pick. You can go back, or add agents later.</p>
      )}
      <DeciderPicker kinds={view.kinds} value={choice} busy={busy} explainFallback
        onSelect={id => { setChoice(id); setTried(null) }}
        onSaveKey={async (id, key) => { await guard(async () => { await setWardenKey(id, key); await reload(); setTried(null) }) }} />

      <div className="onboard-try">
        <span className="onboard-try__label">TRY IT</span>
        <div className="warden-samples">
          {WARDEN_SAMPLES.map(text => (
            <button key={text} type="button" className="warden-sample" disabled={busy} onClick={() => void tryIt(text)}>{text}</button>
          ))}
        </div>
        {/* Always there, so an answer arriving does not push the buttons down. */}
        <div className={`onboard-try__result${tried && !asking ? '' : ' is-empty'}`} role="status">
          {asking ? <span>Asking…</span>
            : tried ? <>
              <strong>{capitalize(describeDecision(tried.decision))}</strong>
              <span>
                {tried.decision.fallback
                  ? `Built-in rules decided: ${tried.decision.fallback}`
                  : `${tried.decision.decider}${tried.decision.latency_ms !== undefined ? ` · ${tried.decision.latency_ms} ms` : ''}`}
              </span>
            </>
              : <span>Pick a message to see who would answer it.</span>}
        </div>
      </div>

      {error && <p className="onboard__error" role="alert">{error}</p>}
      <div className="onboard__actions">
        <button type="button" className="onboard-btn onboard-btn--primary" disabled={busy} onClick={() => void finish()}>
          TURN ON WARDEN
        </button>
        <div className="onboard__split">
          <button type="button" className="onboard-btn onboard-btn--ghost" disabled={busy} onClick={onBack}>BACK</button>
          <button type="button" className="onboard-btn onboard-btn--ghost" disabled={busy} onClick={onSkip}>NOT NOW</button>
        </div>
      </div>
    </div>
  )
}
