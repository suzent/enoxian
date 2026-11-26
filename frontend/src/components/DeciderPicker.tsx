import { useEffect, useState } from 'react'
import Select from './ui/Select'
import * as api from '../api'
import type { WardenKind } from '../api'

/** The decider a new device is pointed at. */
export const RECOMMENDED_DECIDER = 'jev'

/** Shown as cards; everything else (compatible endpoints, local manifests) sits under "Other". */
const FEATURED = [RECOMMENDED_DECIDER, 'openai', 'anthropic', 'gemini', 'builtin:rules']

const isDecisionApi = (p: WardenKind['protocol']) => p === 'systemone' || p === 'decisions'
const isChatModel = (p: WardenKind['protocol']) =>
  p === 'openai_chat' || p === 'openai_responses' || p === 'anthropic_messages' || p === 'gemini'

/** Messages that show the range of what the Warden does, for "try it". */
export const WARDEN_SAMPLES = [
  'Could someone review the retry logic in src/sync.rs before I merge?',
  'Which of these two approaches is better for caching, and why?',
  'thanks all, heading out for lunch',
]

interface Props {
  kinds: WardenKind[]
  value: string
  busy?: boolean
  onSelect: (id: string) => void
  /** Store a key the person pasted. Keys are never read from the environment here. */
  onSaveKey: (id: string, key: string) => Promise<unknown>
  onRemoveKey?: (id: string) => void
  onKeyDraftChange?: (dirty: boolean) => void
  /** Where requests actually go, when the config overrides the manifest. */
  endpoint?: string | null
  /** Currently configured model, when using a model API. */
  model?: string | null
  onSelectModel?: (model: string) => void
  /** The key comes from an environment variable the person named instead. */
  keyFromEnv?: string | null
  /** Say what happens without a key, where nothing else on the page does. */
  explainFallback?: boolean
}

/**
 * Choose what the Warden asks, and give it a key. Used in onboarding and in
 * settings, so both say the same thing about cost and where messages go.
 */
export default function DeciderPicker({
  kinds, value, busy, onSelect, onSaveKey, onRemoveKey, onKeyDraftChange,
  endpoint, model, onSelectModel, keyFromEnv, explainFallback,
}: Props) {
  const [key, setKey] = useState('')
  const [models, setModels] = useState<string[]>([])
  const [fetchingModels, setFetchingModels] = useState(false)
  // A key typed for one provider is not offered to another. Reset while
  // rendering, only when the choice really changes: an effect would also run
  // on mount, and if that run is still pending when the first keystroke
  // lands, it wipes the key the user just typed.
  const [keyFor, setKeyFor] = useState(value)
  if (keyFor !== value) {
    setKeyFor(value)
    setKey('')
  }
  useEffect(() => { onKeyDraftChange?.(!!key) }, [key, onKeyDraftChange])

  const featured = FEATURED.map(id => kinds.find(k => k.id === id)).filter((k): k is WardenKind => !!k)
  const others = kinds.filter(k => !FEATURED.includes(k.id))
  const selected = kinds.find(k => k.id === value)
  const otherSelected = !!selected && !FEATURED.includes(selected.id)
  const hasKey = !selected?.needs_key || !!selected?.key_hint || !!keyFromEnv
  const hasEndpoint = !!(endpoint || selected?.endpoint)

  useEffect(() => {
    if (!selected || selected.protocol === 'rules' || !hasKey || !hasEndpoint || typeof api.getWardenModels !== 'function') {
      setModels([])
      setFetchingModels(false)
      return
    }
    let live = true
    setFetchingModels(true)
    api.getWardenModels(selected.id, endpoint).then(res => {
      if (!live) return
      const list = res?.models ?? []
      setModels(list)
      if (isChatModel(selected.protocol) && list.length > 0 && (!model || !list.includes(model))) {
        onSelectModel?.(list[0])
      }
    }, () => {
      if (live) setModels([])
    }).finally(() => {
      if (live) setFetchingModels(false)
    })
    return () => { live = false }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected?.id, selected?.key_hint, endpoint, hasKey, hasEndpoint])

  const tag = (k: WardenKind) =>
    k.id === RECOMMENDED_DECIDER ? 'Decision API'
      : isDecisionApi(k.protocol) ? 'Decision API'
        : k.protocol === 'rules' ? 'Free · no key'
          : null

  const card = (k: WardenKind) => {
    const checked = k.id === value
    return (
      <button key={k.id} type="button" role="radio" aria-checked={checked} disabled={busy}
        className={`decider-card${checked ? ' is-selected' : ''}`} onClick={() => !checked && onSelect(k.id)}>
        <span className="decider-card__mark" aria-hidden="true" />
        <span className="decider-card__body">
          <span className="decider-card__title">
            {k.name}
            {tag(k) && <span className="decider-card__tag">{tag(k)}</span>}
            {k.needs_key && k.key_hint && <span className="decider-card__key">Key {k.key_hint}</span>}
          </span>
          <span className="decider-card__text">{k.description}</span>
        </span>
      </button>
    )
  }

  return (
    <div className="decider-picker">
      <div role="radiogroup" aria-label="Decider" className="decider-cards">
        {featured.map(card)}
        {others.length > 0 && (
          <button type="button" role="radio" aria-checked={otherSelected} disabled={busy}
            className={`decider-card${otherSelected ? ' is-selected' : ''}`}
            onClick={() => !otherSelected && onSelect(others[0].id)}>
            <span className="decider-card__mark" aria-hidden="true" />
            <span className="decider-card__body">
              <span className="decider-card__title">Other provider</span>
              <span className="decider-card__text">OpenAI-compatible endpoint, OpenAI Decisions, or custom manifest.</span>
            </span>
          </button>
        )}
      </div>

      {otherSelected && others.length > 1 && (
        <label className="decider-field">
          Provider
          <Select aria-label="Other provider" disabled={busy} value={value}
            options={others.map(k => ({ value: k.id, label: `${k.name}${isDecisionApi(k.protocol) ? ' (Decision API)' : ''}` }))} onChange={onSelect} />
        </label>
      )}

      {/* One box whatever is chosen, so switching cards does not move what follows. */}
      <div className="decider-detail">
      {selected?.needs_key && !keyFromEnv && (
        <div className="decider-key">
          <label className="decider-field" htmlFor="decider-key-input">
            <span>
              {selected.name} API key
              <span className="decider-key__state">{selected.key_hint ? ` · saved ${selected.key_hint}` : ' · not set'}</span>
            </span>
            {selected.key_url && (
              <a href={selected.key_url} target="_blank" rel="noreferrer" className="decider-key__link">Get a key ↗</a>
            )}
          </label>
          <form className="decider-key__row" onSubmit={e => {
            e.preventDefault()
            if (!key.trim()) return
            void onSaveKey(selected.id, key.trim()).then(() => setKey(''), () => {})
          }}>
            <input id="decider-key-input" type="password" value={key} disabled={busy} autoComplete="off" spellCheck={false}
              aria-label="API key" placeholder={selected.key_hint ? 'Paste a new key to replace it' : 'Paste your key'}
              onChange={e => setKey(e.target.value)} />
            <button type="submit" className="enox-btn" disabled={busy || !key.trim()}>Save key</button>
            {selected.key_hint && onRemoveKey && (
              <button type="button" className="enox-btn" disabled={busy} onClick={() => onRemoveKey(selected.id)}>Remove key</button>
            )}
          </form>
          {models.length > 0 && (
            <label className="decider-field">
              <span>Model</span>
              <Select aria-label="Fetched model" disabled={busy} value={model || models[0] || ''}
                options={models.map(m => ({ value: m, label: m }))}
                onChange={m => onSelectModel?.(m)} />
            </label>
          )}
          {fetchingModels && models.length === 0 && (
            <p className="decider-note">Fetching models…</p>
          )}
          <p className="decider-note">
            {explainFallback && !selected.key_hint && <>Until a key is saved, the built-in rules decide. </>}
            Stored locally; a CLI subscription login does not work here.
          </p>
          {!endpoint && !selected.endpoint && (
            <p className="decider-note">Set its endpoint and model under Connection details in the Warden settings.</p>
          )}
        </div>
      )}
      {selected?.needs_key && keyFromEnv && (
        <p className="decider-note">Key read from <code>{keyFromEnv}</code> on this device.</p>
      )}
      {selected?.protocol === 'rules' && (
        <p className="decider-note">Local only; decides by message length.</p>
      )}
      {selected && !selected.needs_key && selected.protocol !== 'rules' && (
        <p className="decider-note">No key needed.</p>
      )}
      </div>
    </div>
  )
}
