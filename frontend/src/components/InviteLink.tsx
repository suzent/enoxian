import { useState } from 'react'

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

  return <>
    <div className="panel-action-form__heading">
      <strong>INVITE LINK</strong>
      <span>Share with a trusted collaborator</span>
    </div>
    {canSwitch && <label className="panel-action-form__field invite-link-option">
      <input type="checkbox" checked={useFull} onChange={e => {
        setUseFull(e.target.checked)
        setCopiedUri(null)
        setError(null)
      }} /> Use full invite
    </label>}
    <div className="panel-action-form__inline">
      <input className="panel-action-form__value invite-link-value" aria-label="Invite link" readOnly value={shown}
        onFocus={e => e.currentTarget.select()} />
      <button onClick={copy}
        className={`panel-action-form__button panel-action-form__button--primary${copiedUri === shown ? ' is-complete' : ''}`}
      >{copiedUri === shown ? 'COPIED ✓' : 'COPY'}</button>
    </div>
    <p className="invite-link-note">{note || (shown.startsWith('enoxian://s1/')
      ? 'Short invite · Requires the relay when joining.'
      : 'Full invite · No relay needed to retrieve invitation details.')}</p>
    {error && <p role="alert">{error}</p>}
  </>
}
