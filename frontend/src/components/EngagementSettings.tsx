import type { CircleSettingsView, SettingsView } from '../types'

/** One setting, at one scope. `null` means "inherit" and is only offered on a
 *  Circle — the global scope has nothing above it to inherit from. */
export type Patch = {
  reaction?: 'push' | 'pull' | null
  engagement_window_secs?: number | null
  ambient?: string[] | null
  max_relay_turns?: number | null
}

interface Props {
  /** Agents this device can actually run — the only ones worth listing. */
  agentNames: string[]
  /** What applies everywhere. */
  global: SettingsView
  /** The Circle being edited, or null for the global scope. */
  circle: CircleSettingsView | null
  /** Which scope the controls edit. */
  scope: 'global' | 'circle'
  busy: boolean
  onChange: (patch: Patch) => void
}

/**
 * Engagement settings for one scope.
 *
 * There is deliberately no per-agent "accepts hand-offs" switch. An agent
 * allowed into a Circle is reachable by the other agents in it; what remains
 * configurable is how far a chain may run, not who may start one.
 */
export default function EngagementSettings({
  agentNames, global, circle, scope, busy, onChange,
}: Props) {
  const isCircle = scope === 'circle'
  const over = circle?.overrides ?? {}
  const effective = isCircle ? circle?.effective ?? global : global

  // At Circle scope a setting is either overridden or inherited, and the UI has
  // to show which — an inherited value that looks set is how you end up
  // changing the wrong scope.
  const inherits = (key: keyof Patch) => isCircle && over[key] === undefined

  const row = (key: keyof Patch, label: string, control: React.ReactNode, hint: string) => (
    <div className={`engagement-row${inherits(key) ? ' engagement-row--inherited' : ''}`}>
      <div className="engagement-row__label">
        <span>{label}</span>
        {isCircle && (
          inherits(key)
            ? <span className="engagement-row__badge">inherited</span>
            : <button
                type="button"
                className="engagement-row__reset"
                disabled={busy}
                onClick={() => onChange({ [key]: null } as Patch)}
                title="Stop overriding this here and follow the global setting"
              >reset</button>
        )}
      </div>
      <div className="engagement-row__control">{control}</div>
      <div className="engagement-row__hint">{hint}</div>
    </div>
  )

  return (
    <div className="engagement-settings">
      {row('reaction', 'Run agents when mentioned',
        <label className="engagement-toggle">
          <input
            type="checkbox"
            checked={effective.reaction === 'push'}
            disabled={busy}
            onChange={e => {
              if (e.target.checked && !window.confirm(
                'Run agents on mention?\n\nAnyone in this Circle who @mentions one of your agents ' +
                'can start it as a process on THIS machine.',
              )) return
              onChange({ reaction: e.target.checked ? 'push' : 'pull' })
            }}
          />
          <span>{effective.reaction === 'push' ? 'on' : 'off'}</span>
        </label>,
        effective.reaction === 'push'
          ? 'A mention starts the agent here.'
          : 'Mentions never start anything on this machine.')}

      {row('engagement_window_secs', 'Follow-up window',
        <span className="engagement-number">
          <input
            type="number" min={0} step={30}
            value={effective.engagement_window_secs}
            disabled={busy}
            onChange={e => onChange({ engagement_window_secs: Math.max(0, Number(e.target.value) || 0) })}
            aria-label="Follow-up window in seconds"
          />
          <span>sec</span>
        </span>,
        effective.engagement_window_secs > 0
          ? `Replying to an agent needs no mention for ${effective.engagement_window_secs}s.`
          : 'Off — every message needs an explicit @mention.')}

      {row('max_relay_turns', 'Hand-off chain limit',
        <span className="engagement-number">
          <input
            type="number" min={1} max={50}
            value={effective.max_relay_turns}
            disabled={busy}
            onChange={e => onChange({ max_relay_turns: Math.max(1, Number(e.target.value) || 1) })}
            aria-label="Maximum agent turns in one chain"
          />
          <span>turns</span>
        </span>,
        `Agents may hand work to each other; a chain stops after ${effective.max_relay_turns} turns.`)}

      {row('ambient', 'Agents that read the room',
        <div className="engagement-agents">
          {agentNames.length === 0 && <span className="engagement-row__hint">No agents configured.</span>}
          {agentNames.map(name => {
            const on = effective.ambient.includes(name)
            return (
              <label key={name} className="engagement-toggle">
                <input
                  type="checkbox"
                  checked={on}
                  disabled={busy}
                  onChange={() => {
                    if (!on && !window.confirm(
                      `Let @${name} read the room?\n\n` +
                      'Every message anyone types here will be sent to this agent\'s model ' +
                      'provider — not just the ones addressed to it. Everyone in the Circle can ' +
                      'see that it is listening.',
                    )) return
                    onChange({
                      ambient: on
                        ? effective.ambient.filter(a => a !== name)
                        : [...effective.ambient, name],
                    })
                  }}
                />
                <span>@{name}</span>
              </label>
            )
          })}
        </div>,
        effective.ambient.length === 0
          ? 'Nobody — agents answer only when addressed.'
          : 'These answer messages that name no agent, and stay quiet otherwise.')}
    </div>
  )
}
