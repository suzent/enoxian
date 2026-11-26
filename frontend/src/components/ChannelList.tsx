import { useState } from 'react'
import { Archive, Pencil, Plus } from 'lucide-react'
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
  onRename: (id: string, name: string) => Promise<void>
}

const errorText = (err: unknown) => (err instanceof Error ? err.message : String(err))

/** The active circle's topic channels, nested under its row in the sidebar.
 *  `#general` always comes first and cannot be renamed or archived. */
export default function ChannelList({ channels, current, unread, onSelect, onCreate, onArchive, onRename }: Props) {
  const [creating, setCreating] = useState(false)
  const [name, setName] = useState('')
  const [purpose, setPurpose] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  // The channel being renamed in place, and its draft name.
  const [renaming, setRenaming] = useState<{ id: string; value: string; error: string | null } | null>(null)
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
      setError(errorText(err))
    } finally {
      setBusy(false)
    }
  }

  const startRename = (c: Channel) => {
    if (c.id === null || c.archived) return
    setRenaming({ id: c.id, value: c.name, error: null })
  }

  const commitRename = async (original: string) => {
    if (!renaming || busy) return
    const next = renaming.value.trim().replace(/^#/, '')
    if (!next || next === original) { setRenaming(null); return }
    setBusy(true)
    try {
      await onRename(renaming.id, next)
      setRenaming(null)
    } catch (err) {
      setRenaming(r => (r ? { ...r, error: errorText(err) } : r))
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
          const editable = c.id !== null && !c.archived
          if (renaming && renaming.id === c.id) {
            return (
              <li key={key} className="channel-row is-renaming">
                <form
                  className="channel-rename"
                  onSubmit={event => { event.preventDefault(); void commitRename(c.name) }}
                >
                  <span className="channel-row__hash" aria-hidden="true">#</span>
                  <input
                    autoFocus
                    aria-label={`Rename #${c.name}`}
                    value={renaming.value}
                    maxLength={33}
                    spellCheck={false}
                    onFocus={event => event.currentTarget.select()}
                    onChange={event => setRenaming({ ...renaming, value: event.target.value, error: null })}
                    onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); setRenaming(null) } }}
                    onBlur={() => { if (!busy) setRenaming(null) }}
                  />
                  <span className="channel-rename__hint" aria-hidden="true">↵</span>
                </form>
                {renaming.error && <span className="channel-rename__error" role="alert">{renaming.error}</span>}
              </li>
            )
          }
          return (
            <li key={key || 'general'} className={`channel-row${selected ? ' is-current' : ''}${count > 0 ? ' has-unread' : ''}${c.archived ? ' is-archived' : ''}${editable ? ' is-editable' : ''}`}>
              <button
                type="button"
                className="channel-row__select"
                aria-current={selected ? 'true' : undefined}
                title={c.purpose || undefined}
                onClick={() => onSelect(c.id)}
                onDoubleClick={() => startRename(c)}
              >
                <span className="channel-row__hash" aria-hidden="true">#</span>
                <span className="channel-row__name">{c.name}</span>
                {c.archived && <span className="channel-row__tag">archived</span>}
                {count > 0 && <span className="channel-row__count" aria-label={`${count} unread`}>{count > 99 ? '99+' : count}</span>}
              </button>
              {editable && (
                <span className="channel-row__tools">
                  <button
                    type="button"
                    aria-label={`Rename #${c.name}`}
                    title="Rename"
                    onClick={() => startRename(c)}
                  >
                    <Pencil size={11} strokeWidth={1.75} aria-hidden="true" />
                  </button>
                  <button
                    type="button"
                    aria-label={`Archive #${c.name}`}
                    title="Archive"
                    onClick={() => { void onArchive(c.id!).catch(() => {}) }}
                  >
                    <Archive size={11} strokeWidth={1.75} aria-hidden="true" />
                  </button>
                </span>
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
              spellCheck={false}
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
          <Plus size={11} strokeWidth={2} aria-hidden="true" />
          <span>New channel</span>
        </button>
      )}
    </div>
  )
}
