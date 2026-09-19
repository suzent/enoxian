import { useState } from 'react'
import { Copy, Check, Link2 } from 'lucide-react'

export default function InviteLink({ uri, longUri, note }: {
  uri: string
  longUri?: string
  note?: string | null
}) {
  const [useFull, setUseFull] = useState(false)
  const [copiedUri, setCopiedUri] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const shown = useFull && longUri ? longUri : uri
  const canSwitch = !!longUri && longUri !== uri

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(shown)
      setCopiedUri(shown)
      setError(null)
    } catch {
      setError('Unable to copy. Select the invite and copy it manually.')
    }
  }

  return <section className="invite-card" aria-label="Circle invitation">
    <div className="invite-card__heading">
      <Link2 size={16} aria-hidden="true" />
      <strong>Invitation link</strong>
      <span>{shown.startsWith('enoxian://s1/') ? 'SHORT' : 'FULL'}</span>
    </div>
    <p className="invite-card__description">Share this link with your collaborator.</p>
    <div className="invite-card__link">
      <input className="invite-link-value" aria-label="Invite link" readOnly value={shown}
        onFocus={e => e.currentTarget.select()} />
      <button type="button" onClick={copy} aria-label={copiedUri === shown ? 'COPIED ✓' : 'COPY'}
        className={copiedUri === shown ? 'is-complete' : ''}>
        {copiedUri === shown ? <Check size={15} aria-hidden="true" /> : <Copy size={15} aria-hidden="true" />}
        {copiedUri === shown ? 'Copied' : 'Copy'}
      </button>
    </div>
    <span className="sr-only" role="status">{copiedUri === shown ? 'Invitation copied' : ''}</span>
    {canSwitch && <label className="invite-link-option">
      <input type="checkbox" checked={useFull} onChange={e => {
        setUseFull(e.target.checked)
        setCopiedUri(null)
        setError(null)
      }} /> Use full invite
    </label>}
    <p className="invite-card__hint">{shown.startsWith('enoxian://s1/')
      ? 'The relay retrieves this invitation when joining.'
      : 'This link contains the full invitation.'}</p>
    {note && <details className="invite-card__details"><summary>Connection details</summary><p>{note}</p></details>}
    {error && <p className="invite-card__error" role="alert">{error}</p>}
  </section>
}
