import { useEffect, useState } from 'react'
import type { IdentityInfo } from '../api'
import type { AddressedAs } from '../types'

interface Props {
  identity: IdentityInfo
  /** How the active Circle addresses this device, if there is one. */
  addressedAs?: AddressedAs | null
  /** Name of that Circle, for the label. */
  circleName?: string | null
  configPath?: string | null
  onDirtyChange?: (dirty: boolean) => void
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
  identity, addressedAs, circleName, configPath, busy, onSave, onDirtyChange,
}: Props) {
  const [label, setLabel] = useState(identity.device_label)
  const [handle, setHandle] = useState(identity.user_handle ?? '')

  const labelChanged = label.trim() !== identity.device_label
  const handleChanged = handle.trim() !== (identity.user_handle ?? '')
  const dirty = (labelChanged || handleChanged) && label.trim().length > 0

  useEffect(() => { onDirtyChange?.(labelChanged || handleChanged) }, [labelChanged, handleChanged, onDirtyChange])

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
      <section className="settings-option-group">
        <div className="engagement-row">
          <div className="engagement-row__label"><span>Device name</span></div>
          <div className="engagement-row__control device-identity__control">
            <input
              value={label}
              disabled={busy}
              onChange={e => setLabel(e.target.value)}
              placeholder="macbook-pro"
              aria-label="Device name"
            />
          </div>
          <div className="engagement-row__hint">
            {owner ? (
              <>
                {circleName ? <>In <strong>{circleName}</strong>, agents</> : 'Agents'} here are addressed as{' '}
                <code>@{owner}/{deviceSegment}/agent</code>.
                {labelChanged && ' Saving updates that for everyone.'}
              </>
            ) : (
              <>Addressed as <code>@{handle.trim() || 'you'}/{deviceSegment}/agent</code>.</>
            )}
          </div>
        </div>

        <div className="engagement-row">
          <div className="engagement-row__label"><span>Your handle</span></div>
          <div className="engagement-row__control device-identity__control">
            <input
              value={handle}
              disabled={busy}
              onChange={e => setHandle(e.target.value)}
              placeholder="your handle"
              aria-label="User handle"
            />
          </div>
          <div className="engagement-row__hint">
            Used when you <em>create or join</em> a Circle; existing Circles keep
            {owner ? <> <strong>{owner}</strong>.</> : ' your old handle.'}
          </div>
        </div>

        <div className="device-identity__footer">
          <button type="button" className="enox-btn" onClick={save} disabled={!dirty || busy}>
            {busy ? 'SAVING…' : 'SAVE'}
          </button>
        </div>
      </section>

      {(configPath || identity.update_channel) && (
        <section className="settings-option-group">
          {configPath && (
            <div className="engagement-row">
              <div className="engagement-row__label"><span>Config file</span></div>
              <div className="engagement-row__hint"><code className="device-identity__code">{configPath}</code></div>
            </div>
          )}
          {identity.update_channel && (
            <div className="engagement-row">
              <div className="engagement-row__label"><span>Update channel</span></div>
              <div className="engagement-row__control">
                <strong className="device-identity__channel">{identity.update_channel}</strong>
              </div>
              <div className="engagement-row__hint">Stream followed by <code>enox update</code>.</div>
            </div>
          )}
        </section>
      )}
    </div>
  )
}
