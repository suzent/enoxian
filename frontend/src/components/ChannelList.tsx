import { useState } from 'react'
import { Archive, Plus } from 'lucide-react'
import type { Channel } from '../types'
import { channelKey } from '../context/ChannelContext'

interface Props {
  channels: Channel[]
  current: string | null
  /** Messages in each channel this viewer has not opened, by `channelKey`. */
  unread: Record<string, number>
  onSelect: (id: string | null) => void
  onCreate: (name: string, purpose: string) => Promise<void>
  onArchive: (id: string) => Promise<void>
}

/** The active circle's topic channels, nested under its row in the sidebar.
 *  `#general` always comes first and cannot be archived. */
export default function ChannelList({ channels, current, unread, onSelect, onCreate, onArchive }: Props) {
  const [creating, setCreating] = useState(false)
  const [name, setName] = useState('')
  const [purpose, setPurpose] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // An archived channel stays listed only while it is open.
  const listed = channels.filter(c => !c.archived || c.id === current)

  const close = () => {
    setCreating(false)
    setName('')
    setPurpose('')
    setError(null)
  }

  const submit = async () => {
    if (!name.trim() || busy) return
    setBusy(true)
    setError(null)
    try {
      await onCreate(name.trim(), purpose.trim())
      close()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="channel-list">
      <ul className="channel-list__items" aria-label="Channels">
        {listed.map(c => {
          const key = channelKey(c.id)
          const selected = c.id === current
          const count = selected ? 0 : unread[key] ?? 0
          return (
            <li key={key || 'general'} className={`channel-row${selected ? ' is-current' : ''}${count > 0 ? ' has-unread' : ''}${c.archived ? ' is-archived' : ''}`}>
              <button
                type="button"
                className="channel-row__select"
                aria-current={selected ? 'true' : undefined}
                title={c.purpose || undefined}
                onClick={() => onSelect(c.id)}
              >
                <span className="channel-row__hash" aria-hidden="true">#</span>
                <span className="channel-row__name">{c.name}</span>
                {c.archived && <span className="channel-row__tag">archived</span>}
                {count > 0 && <span className="channel-row__count" aria-label={`${count} unread`}>{count > 99 ? '99+' : count}</span>}
              </button>
              {c.id !== null && !c.archived && (
                <button
                  type="button"
                  className="channel-row__archive"
                  aria-label={`Archive #${c.name}`}
                  title={`Archive #${c.name}`}
                  onClick={() => { void onArchive(c.id!).catch(() => {}) }}
                >
                  <Archive size={12} strokeWidth={1.75} aria-hidden="true" />
                </button>
              )}
            </li>
          )
        })}
      </ul>

      {creating ? (
        <form
          className="channel-form"
          onSubmit={event => { event.preventDefault(); void submit() }}
          onKeyDown={event => { if (event.key === 'Escape') close() }}
        >
          <label className="channel-form__name">
            <span aria-hidden="true">#</span>
            <input
              autoFocus
              aria-label="Channel name"
              placeholder="name"
              value={name}
              maxLength={33}
              onChange={event => setName(event.target.value)}
            />
          </label>
          <input
            className="channel-form__purpose"
            aria-label="Channel purpose"
            placeholder="purpose (optional)"
            value={purpose}
            maxLength={200}
            onChange={event => setPurpose(event.target.value)}
          />
          {error && <span className="channel-form__error" role="alert">{error}</span>}
          <div className="channel-form__actions">
            <button type="button" className="channel-form__cancel" onClick={close}>Cancel</button>
            <button type="submit" className="channel-form__create" disabled={busy || !name.trim()}>Create</button>
          </div>
        </form>
      ) : (
        <button type="button" className="channel-list__new" onClick={() => setCreating(true)}>
          <Plus size={12} strokeWidth={2} aria-hidden="true" />
          <span>New channel</span>
        </button>
      )}
    </div>
  )
}
