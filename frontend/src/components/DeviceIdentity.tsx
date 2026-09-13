import { useState } from 'react'
import type { IdentityInfo } from '../api'
import type { AddressedAs } from '../types'

interface Props {
  identity: IdentityInfo
  /** How the active Circle addresses this device, if there is one. */
  addressedAs?: AddressedAs | null
  /** Name of that Circle, for the label. */
  circleName?: string | null
  busy: boolean
  onSave: (patch: { device_label?: string; user_handle?: string }) => Promise<void>
}

/**
 * Who this machine is.
 *
 * Two identities meet here and they are not the same, which is the thing this
 * component mostly exists to keep straight:
 *
 * - **device name** — local, and the middle segment of every handle addressing
 *   an agent here. Renaming it changes that handle everywhere.
 * - **your handle** — local too, but only used when you *create or join* a
 *   Circle. Your name inside a Circle you already joined is part of your
 *   membership there and does not change when you edit this.
 *
 * So the address shown is the one the Circle actually uses, read from its
 * roster, rather than one assembled from local fields. A handle that looks
 * authoritative and is wrong is the worst outcome: a mention to it fails
 * silently.
 */
export default function DeviceIdentity({
  identity, addressedAs, circleName, busy, onSave,
}: Props) {
  const [label, setLabel] = useState(identity.device_label)
  const [handle, setHandle] = useState(identity.user_handle ?? '')

  const labelChanged = label.trim() !== identity.device_label
  const handleChanged = handle.trim() !== (identity.user_handle ?? '')
  const dirty = (labelChanged || handleChanged) && label.trim().length > 0

  const save = async () => {
    if (!dirty) return
    await onSave({
      ...(labelChanged ? { device_label: label.trim() } : {}),
      ...(handleChanged ? { user_handle: handle.trim() } : {}),
    })
  }

  // The owner segment comes from the Circle, never from the local handle. Only
  // the device segment previews the pending rename, because that one really
  // does follow this field.
  const owner = addressedAs?.owner
  const deviceSegment = label.trim() || 'device'

  return (
    <div className="device-identity">
      <label className="device-identity__field">
        <span>Device name</span>
        <input
          value={label}
          disabled={busy}
          onChange={e => setLabel(e.target.value)}
          placeholder="macbook-pro"
          aria-label="Device name"
        />
      </label>
      <label className="device-identity__field">
        <span>Your handle</span>
        <input
          value={handle}
          disabled={busy}
          onChange={e => setHandle(e.target.value)}
          placeholder="your handle"
          aria-label="User handle"
        />
      </label>

      <div className="device-identity__hint">
        {owner ? (
          <>
            {circleName ? <>In <strong>{circleName}</strong>, agents</> : 'Agents'} on this machine
            are addressed as <code>@{owner}/{deviceSegment}/agent</code>.
            {labelChanged && ' Saving updates that for everyone.'}
          </>
        ) : (
          <>Agents on this machine are addressed as{' '}
          <code>@{handle.trim() || 'you'}/{deviceSegment}/agent</code>.</>
        )}
      </div>

      <div className="device-identity__hint">
        {/* The distinction people get wrong: "change your handle" reads like it
            changes everywhere, and it does not. */}
        Your handle is used when you <em>create or join</em> a Circle. Changing
        it here does not rename you in Circles you have already joined
        {owner ? <> — you stay <strong>{owner}</strong> there.</> : '.'}
      </div>

      <div className="device-identity__actions">
        <button type="button" onClick={save} disabled={!dirty || busy}>
          {busy ? 'SAVING…' : 'SAVE'}
        </button>
        {identity.update_channel && (
          <span className="device-identity__channel">
            updates: <strong>{identity.update_channel}</strong>
          </span>
        )}
      </div>
    </div>
  )
}
