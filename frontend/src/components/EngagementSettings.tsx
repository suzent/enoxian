import type { CircleSettingsView, SettingsView } from '../types'

/** One setting, at one scope. `null` means "inherit" and is only offered on a
 *  Circle — the global scope has nothing above it to inherit from. */
export type Patch = {
  ambient_responders?: number | null
  ambient_rotate_count?: boolean | null
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
            ? <span className="engagement-row__badge">Using default</span>
            : <button
                type="button"
                className="engagement-row__reset"
                disabled={busy}
                onClick={() => onChange({ [key]: null } as Patch)}
                title="Remove this Circle override and use the default for all Circles"
              >Use default</button>
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
            aria-label="Run agents when mentioned"
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
          ? 'Mentions and enabled read-the-room agents can start runs here.'
          : 'Automatic replies are paused on this device, including read-the-room.')}

      {row('engagement_window_secs', 'Unthreaded follow-ups',
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
          ? `A new, unaddressed message may go to the last agent for ${effective.engagement_window_secs}s. Use Reply to choose an agent explicitly.`
          : 'Use Reply on an agent’s message or @mention it. New messages do not automatically go to the last speaker.')}

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
                      'New, unaddressed human messages may be sent to this agent’s model provider. ' +
                      'Selected listeners can run, which uses their model providers. ' +
                      'The agent may reply or choose to stay quiet. Everyone can see that it is listening.',
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
          : effective.reaction !== 'push'
            ? 'Listening is paused. Turn on automatic agent runs above to activate these listeners.'
            : 'Listeners take turns on new, unaddressed human messages. Those selected can reply or stay quiet; the others wait for a later message. Short messages, recent speakers, and agent messages are skipped.')}
      {row('ambient_responders', 'Listeners per message',
        <input type="number" min={1} max={32} aria-label="Listeners per message" className="border px-2 py-1 w-20"
          value={effective.ambient_responders ?? 1} disabled={busy}
          onChange={e => onChange({ ambient_responders: Math.min(32, Math.max(1, Number(e.target.value) || 1)) })} />,
        'Choose how many listeners may consider each message on this device. Agents take turns fairly; each may reply or stay quiet. Selected requests queue when execution slots are full.')}
      {row('ambient_rotate_count', 'Vary the number of listeners',
        <label className="engagement-toggle"><input type="checkbox" aria-label="Vary the number of listeners"
          checked={effective.ambient_rotate_count ?? false} disabled={busy}
          onChange={e => onChange({ ambient_rotate_count: e.target.checked })} /><span>{effective.ambient_rotate_count ? 'on' : 'off'}</span></label>,
        effective.ambient_rotate_count
          ? `Cycle through 1 to ${effective.ambient_responders ?? 1} listeners across messages, limited by who is eligible.`
          : 'Keep the same listener limit while rotating which agents get a turn.')}


    </div>
  )
}
