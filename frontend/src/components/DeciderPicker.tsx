import { useEffect, useState } from 'react'
import Select from './ui/Select'
import type { WardenKind } from '../api'

/** The decider a new device is pointed at. */
export const RECOMMENDED_DECIDER = 'jev'

/** Shown as cards; everything else (proxies, local manifests) sits under "Other". */
const FEATURED = [RECOMMENDED_DECIDER, 'openai-decisions', 'builtin:rules']

/** Messages that show the range of what the Warden does, for "try it". */
export const WARDEN_SAMPLES = [
  'Could someone review the retry logic in src/sync.rs before I merge?',
  'Which of these two approaches is better for caching, and why?',
  'thanks all, heading out for lunch',
]

const host = (url?: string | null) => {
  try { return url ? new URL(url).host : null } catch { return null }
}

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
  /** The key comes from an environment variable the person named instead. */
  keyFromEnv?: string | null
  /** Say what happens without a key, where nothing else on the page does. */
  explainFallback?: boolean
}

/**
 * Choose what the Warden asks, and give it a key. Used in onboarding and in
 * settings, so both say the same thing about cost and where messages go.
 */
export default function DeciderPicker({ kinds, value, busy, onSelect, onSaveKey, onRemoveKey, onKeyDraftChange, endpoint, keyFromEnv, explainFallback }: Props) {
  const [key, setKey] = useState('')
  useEffect(() => { setKey('') }, [value])
  useEffect(() => { onKeyDraftChange?.(!!key) }, [key, onKeyDraftChange])

  const featured = FEATURED.map(id => kinds.find(k => k.id === id)).filter((k): k is WardenKind => !!k)
  const others = kinds.filter(k => !FEATURED.includes(k.id))
  const selected = kinds.find(k => k.id === value)
  const otherSelected = !!selected && !FEATURED.includes(selected.id)

  const tag = (k: WardenKind) =>
    k.id === RECOMMENDED_DECIDER ? 'Recommended'
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

  const destination = host(endpoint || selected?.endpoint)

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
              <span className="decider-card__text">A LiteLLM proxy, or a decider you added under ~/.enoxian/wardens.</span>
            </span>
          </button>
        )}
      </div>

      {otherSelected && others.length > 1 && (
        <label className="decider-field">
          Provider
          <Select aria-label="Other provider" disabled={busy} value={value}
            options={others.map(k => ({ value: k.id, label: k.name }))} onChange={onSelect} />
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
          <p className="decider-note">
            {explainFallback && !selected.key_hint && <>Until a key is saved, the built-in rules decide. </>}
            Stored only on this device and never shown again.
            {destination && <> In Circles where the Warden is on, messages nobody addressed are sent to <strong>{destination}</strong> to decide who answers.</>}
          </p>
        </div>
      )}
      {selected?.needs_key && keyFromEnv && (
        <p className="decider-note">Key read from <code>{keyFromEnv}</code> on this device.</p>
      )}
      {selected?.protocol === 'rules' && (
        <p className="decider-note">Nothing leaves this device. Rules work on message length alone, so they cannot tell who is best placed to answer.</p>
      )}
      {selected && !selected.needs_key && selected.protocol !== 'rules' && (
        <p className="decider-note">No key needed.</p>
      )}
      </div>
    </div>
  )
}
