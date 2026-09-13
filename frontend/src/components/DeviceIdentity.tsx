import { useState } from 'react'
import type { IdentityInfo } from '../api'

interface Props {
  identity: IdentityInfo
  busy: boolean
  onSave: (patch: { device_label?: string; user_handle?: string }) => Promise<void>
}

/**
 * Who this machine is in a Circle.
 *
 * Not cosmetic: the device label is the middle segment of every handle that
 * addresses an agent here (`@owner/device/agent`), and it is what this device
 * compares an incoming mention against. Renaming it changes how the Circle
 * reaches you, which is why it says so rather than presenting a bare field.
 */
export default function DeviceIdentity({ identity, busy, onSave }: Props) {
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

  const example = `@${handle.trim() || 'you'}/${label.trim() || 'device'}/agent`

  return (
    <div className="device-identity">
      <label className="device-identity__field">
        <span>THIS DEVICE</span>
        <input
          value={label}
          disabled={busy}
          onChange={e => setLabel(e.target.value)}
          placeholder="macbook-pro"
          aria-label="Device name"
        />
      </label>
      <label className="device-identity__field">
        <span>YOU</span>
        <input
          value={handle}
          disabled={busy}
          onChange={e => setHandle(e.target.value)}
          placeholder="your handle"
          aria-label="User handle"
        />
      </label>

      <div className="device-identity__hint">
        Agents on this machine are addressed as <code>{example}</code>. Renaming
        changes that handle for everyone in your Circles.
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
