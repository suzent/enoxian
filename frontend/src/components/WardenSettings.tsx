import { useCallback, useEffect, useRef, useState } from 'react'
import DeciderPicker, { WARDEN_SAMPLES } from './DeciderPicker'
import NumberField from './ui/NumberField'
import { ConfirmDialog } from './ui/Dialog'
import {
  getWarden, resetWardenSpec, setWardenConfig, setWardenKey, setWardenSpec, testWarden,
  type WardenDecision, type WardenView,
} from '../api'

/** One line for a decision: what it chose, and why. */
export function describeDecision(d: WardenDecision): string {
  const c = d.choice
  const what = !c.admit || !c.mode
    ? 'nobody answers'
    : c.mode === 'vote'
      ? `blind vote, ${c.listeners}${c.max_listeners > c.listeners ? `–${c.max_listeners}` : ''} voters`
      : c.mode === 'room'
        ? `${c.listeners} listeners together`
        : `one listener (${c.agents[0] ?? '?'})`
  return `${what} — ${d.reason}`
}

const pct = (p?: number) => (p === undefined ? '–' : `${Math.round(p * 100)}%`)

function DecisionView({ decision, questions }: { decision: WardenDecision; questions?: string[] }) {
  return (
    <div className="font-mono text-[10px] border border-obsidian/30 p-2 mt-2 space-y-1">
      <div className="text-[11px] font-bold">{describeDecision(decision)}</div>
      <div className="text-slate">
        {decision.decider}
        {decision.latency_ms !== undefined && ` · ${decision.latency_ms} ms`}
        {decision.cost_usd !== undefined && ` · $${decision.cost_usd.toFixed(5)}`}
        {decision.input_tokens !== undefined && ` · ${decision.input_tokens} tokens in`}
        {decision.cached_tokens !== undefined && ` (${decision.cached_tokens} cached)`}
      </div>
      {decision.fallback && <div role="alert">The rules decided instead: {decision.fallback}</div>}
      {decision.p_admit !== undefined && <div>Needs an answer: {pct(decision.p_admit)}</div>}
      {decision.p_success && Object.keys(decision.p_success).length > 0 && (
        <div>
          Would succeed:{' '}
          {Object.entries(decision.p_success).map(([mode, p]) => (
            <span key={mode} className="mr-2">{mode} {pct(p)}
              {decision.scores?.[mode] !== undefined && <span className="text-slate"> (score {decision.scores[mode].toFixed(2)})</span>}
            </span>
          ))}
        </div>
      )}
      {decision.choice.agents.length > 1 && (
        <div>
          Order:{' '}
          {decision.choice.agents.map(a => (
            <span key={a} className="mr-2">@{a}{decision.p_agent?.[a] !== undefined && ` ${pct(decision.p_agent[a])}`}</span>
          ))}
        </div>
      )}
      {questions && <div className="text-slate">Asked: {questions.join(', ')}</div>}
    </div>
  )
}

/**
 * The warden's device-wide settings: which decider it asks, its key, the spec
 * it decides by, and a way to try it on a sample message. Whether it acts in a
 * Circle is set per Circle, under Behaviour.
 */
export default function WardenSettings({ circleId, onDirtyChange, onBusyChange }: { circleId?: string | null; onDirtyChange?: (dirty: boolean) => void; onBusyChange?: (busy: boolean) => void }) {
  const [view, setView] = useState<WardenView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [draft, setDraft] = useState<{ endpoint: string; model: string; key_env: string; timeout_secs: number }>({
    endpoint: '', model: '', key_env: '', timeout_secs: 5,
  })
  const [keyDirty, setKeyDirty] = useState(false)
  const [specText, setSpecText] = useState('')
  const [sample, setSample] = useState('')
  const [asAgent, setAsAgent] = useState(false)
  const [tried, setTried] = useState<{ decision: WardenDecision; questions: string[] } | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const [confirmation, setConfirmation] = useState<{ title: string; detail: string; action: () => void } | null>(null)
  const loadingId = useRef(0)
  const running = useRef(false)
  const configDraft = (v: WardenView) => ({ endpoint: v.config.endpoint ?? '', model: v.config.model ?? '', key_env: v.config.key_env ?? '', timeout_secs: v.config.timeout_secs })
  const configDirty = !!view && JSON.stringify(draft) !== JSON.stringify(configDraft(view))
  const specDirty = !!view && specText !== view.spec_toml
  useEffect(() => { onDirtyChange?.(configDirty || specDirty || keyDirty) }, [configDirty, specDirty, keyDirty, onDirtyChange])
  useEffect(() => { onBusyChange?.(busy) }, [busy, onBusyChange])

  const refresh = useCallback(async () => {
    const request = ++loadingId.current
    setError(null)
    try {
      const v = await getWarden()
      if (request !== loadingId.current) return
      setView(v)
      setDraft(configDraft(v))
      setSpecText(v.spec_toml)
    } catch (e) { if (request === loadingId.current) setError(e instanceof Error ? e.message : 'Unable to load Warden settings') }
  }, [])
  useEffect(() => { void refresh(); return () => { loadingId.current += 1 } }, [refresh])

  const run = async (fn: () => Promise<unknown>, done?: string, reload: 'none' | 'view' | 'config' | 'spec' = 'view') => {
    if (running.current) return false
    running.current = true
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      await fn()
      if (reload !== 'none') {
        const v = await getWarden()
        setView(v)
        // Refresh only what this action saved. Other drafts belong to the user.
        if (reload === 'config') setDraft(configDraft(v))
        if (reload === 'spec') setSpecText(v.spec_toml)
      }
      if (done) setNotice(done)
      return true
    } catch (e) { setError(e instanceof Error ? e.message : 'Unable to update Warden'); return false }
    finally { running.current = false; setBusy(false) }
  }

  if (!view) return <div className="warden-loading">{error ? <><p role="alert">{error}</p><button className="enox-btn" onClick={() => void refresh()}>Try again</button></> : <p role="status">Loading Warden settings…</p>}</div>
  const kind = view.kinds.find(k => k.id === view.config.decider)
  const asksModel = kind && kind.protocol !== 'rules'
  // A proxy is nothing without its address; a hosted decider needs only a key.
  const proxy = !!kind && asksModel && !kind.key_url

  return (
    <section className="warden-settings">
      <p className="warden-scope-note">
        The Warden decides which agents, if any, answer a message nobody addressed. These settings stay on this
        device; turn it on per Circle under Behaviour.
      </p>
      <section className="warden-group">
      <h3>Decision provider</h3>
      <p className={`warden-status${view.problem ? ' is-problem' : ''}`} role={view.problem ? 'alert' : 'status'}>
        {view.problem
          ? <>Not ready: {view.problem}. The built-in rules decide until it is.</>
          : <>Ready · {kind?.name ?? view.config.decider}</>}
      </p>
      {/* A reserved line, so feedback never pushes the panel down. */}
      <p className={`warden-feedback${error ? ' is-error' : ''}`} role={error ? 'alert' : 'status'} aria-live="polite">
        {error ?? (busy ? 'Working…' : notice) ?? ''}
      </p>

      <DeciderPicker kinds={view.kinds} value={view.config.decider} busy={busy}
        endpoint={view.config.endpoint} keyFromEnv={view.config.key_env}
        onKeyDraftChange={setKeyDirty}
        onSelect={value => {
          const change = () => void run(() => setWardenConfig({ decider: value }), 'Decision provider updated.', 'config')
          if (configDirty || keyDirty) setConfirmation({ title: 'Change decision provider?', detail: 'Unsaved connection settings and the entered key will be discarded. Your policy draft is kept.', action: change })
          else change()
        }}
        onSaveKey={async (id, value) => {
          if (!await run(() => setWardenKey(id, value), 'Key saved on this device.')) throw new Error('not saved')
        }}
        onRemoveKey={id => setConfirmation({ title: 'Remove saved API key?', detail: 'The built-in rules decide until a key is set again.', action: () => void run(() => setWardenKey(id, null), 'Key removed.') })} />

      {/* Kept in place for the rules too, so changing decider does not move the page. */}
      <details className="warden-connection" open={proxy || undefined}>
        <summary>Connection details</summary>
      {!asksModel ? <p className="decider-note py-2">The built-in rules run here and connect to nothing.</p> : (
        <div className="space-y-2 py-2 text-xs">
          <label className="block">Endpoint <span className="text-slate">· {proxy ? "your proxy's address is enough" : 'leave empty for the default'}</span>
            <input className="block w-full border px-2 py-1 mt-1" value={draft.endpoint} disabled={busy}
              placeholder={kind?.endpoint ?? 'https://…'} aria-label="Endpoint"
              onChange={e => setDraft({ ...draft, endpoint: e.target.value })} />
          </label>
          <label className="block">Model
            <input className="block w-full border px-2 py-1 mt-1" value={draft.model} disabled={busy}
              placeholder={kind?.model ?? ''} aria-label="Model"
              onChange={e => setDraft({ ...draft, model: e.target.value })} />
          </label>
          <label className="block">Wait at most
            <NumberField min={1} max={30} className="ml-2 border px-2 py-1 w-16" disabled={busy}
              value={draft.timeout_secs} aria-label="Timeout seconds"
              onCommit={value => setDraft({ ...draft, timeout_secs: value })} />
            <span className="ml-1">seconds, then the rules decide</span>
          </label>
          <label className="block">Read the key from an environment variable instead <span className="text-slate">· optional, by name</span>
            <input className="block w-full border px-2 py-1 mt-1" value={draft.key_env} disabled={busy}
              placeholder="e.g. TYPESAFE_API_KEY" aria-label="Key environment variable"
              onChange={e => setDraft({ ...draft, key_env: e.target.value })} />
          </label>
          <button className="enox-btn text-[9px] px-2 py-1 min-h-0" disabled={busy || !configDirty}
            onClick={() => void run(() => setWardenConfig(draft), 'Connection settings saved.', 'config')}>Save connection</button>
        </div>
      )}
      </details>

      </section>
      <section className="warden-group">
      <h3>Messages to consider</h3>
      <label className="block py-2 text-xs">
        <input type="checkbox" aria-label="Agents' messages too" disabled={busy}
          checked={!!view.config.agent_messages}
          onChange={e => void run(() => setWardenConfig({ agent_messages: e.target.checked }))} />
        <span className="ml-2">Agents' messages too</span>
        <span className="block text-[10px] text-slate mt-1">
          Every new message is the warden's to decide, including one an agent posts naming no one, so
          agents can carry a conversation on without a person in it. What stops them is the hand-off
          limit: each human message pays for at most that many agent turns.
        </span>
        {view.config.agent_messages && (kind?.protocol === 'rules' || view.problem) && (
          <span role="alert" className="block text-[10px] mt-1 border border-obsidian/40 p-2">
            Agents' messages need a decider to judge them. With the built-in rules, or until the decider
            above is usable, no agent's message will be answered.
          </span>
        )}
      </label>

      </section>
      <section className="warden-group">
        <h3>Try a message</h3>
        <p className="warden-hint">Uses your saved settings. Shows a decision without posting to the Circle or running agents.</p>
        <div className="warden-samples">
          {WARDEN_SAMPLES.map(text => (
            <button key={text} type="button" className="warden-sample" disabled={busy} onClick={() => setSample(text)}>{text}</button>
          ))}
        </div>
        <textarea className="block w-full border px-2 py-1 mt-1 text-[11px]" rows={3} value={sample} disabled={busy}
          aria-label="Sample message" placeholder="A message someone might post in the room"
          onChange={e => setSample(e.target.value)} />
        <button className="enox-btn text-[9px] px-2 py-1 min-h-0 mt-1" disabled={busy || !sample.trim()}
          onClick={() => void run(async () => setTried(await testWarden(sample, circleId, asAgent)), undefined, 'none')}>Test decision</button>
        <label className="ml-3 text-[10px]">
          <input type="checkbox" checked={asAgent} disabled={busy} onChange={e => setAsAgent(e.target.checked)} />
          <span className="ml-1">as an agent's message</span>
        </label>
        {tried && <DecisionView decision={tried.decision} questions={tried.questions} />}
      </section>

      <details className="warden-policy warden-group">
        <summary>Advanced policy</summary>
        <div className="flex items-baseline justify-between">
          <span>Spec <span className="text-slate">· {String(view.spec.version)} · {view.spec_source.kind === 'file' ? 'from file' : view.spec_source.kind === 'invalid' ? 'file invalid, using built-in' : 'built-in'}</span></span>
        </div>
        {view.spec_source.kind === 'invalid' && <p role="alert" className="text-[10px]">{view.spec_source.error}</p>}
        <textarea className="block w-full border px-2 py-1 mt-1 text-[10px] font-mono" rows={12} value={specText}
          aria-label="Warden spec" spellCheck={false} disabled={busy}
          onChange={e => setSpecText(e.target.value)} />
        <div className="flex gap-2 mt-1">
          <button className="enox-btn text-[9px] px-2 py-1 min-h-0" disabled={busy || specText === view.spec_toml}
            onClick={() => void run(() => setWardenSpec(specText), 'Policy applied.', 'spec')}>Apply policy</button>
          {view.spec_source.kind !== 'default' && (
            <button className="enox-btn text-[9px] px-2 py-1 min-h-0" disabled={busy}
              onClick={() => setConfirmation({ title: 'Reset to built-in policy?', detail: 'This replaces the saved custom policy and any policy edits here.', action: () => void run(resetWardenSpec, 'Built-in policy restored.', 'spec') })}>Reset to built-in</button>
          )}
        </div>
        <span className="block text-[10px] text-slate mt-1">
          Validated before it is applied. Kept at {view.spec_path ?? '~/.enoxian/warden/spec.toml'}; an
          experiment can replace it with <code>enox warden spec apply</code>.
        </span>
      </details>
      {confirmation && <ConfirmDialog title={confirmation.title} onClose={() => setConfirmation(null)}
        onConfirm={() => { const action = confirmation.action; setConfirmation(null); action() }}>
        {confirmation.detail}
      </ConfirmDialog>}
    </section>
  )
}
