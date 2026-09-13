import type { AgentSummary } from '../types'

interface Props {
  agent: AgentSummary
  busy: boolean
  onChange: (patch: {
    engagement?: 'mention' | 'ambient'
    accept_from?: 'humans' | 'agents'
  }) => void
}

/**
 * How one agent engages: whether it reads unaddressed messages, and whether
 * another agent may hand work to it.
 *
 * Both are off by default and both are device-local. The device that pays for
 * an agent's tokens is the device that decides what they are spent on — nothing
 * here is synced, and no remote peer can change it.
 */
export default function AgentEngagement({ agent, busy, onChange }: Props) {
  const ambient = agent.engagement === 'ambient'
  const acceptsAgents = agent.accept_from === 'agents'

  const toggleAmbient = () => {
    if (!ambient) {
      // Turning this on changes what leaves the Circle, so say so plainly
      // before it does, not after.
      const ok = window.confirm(
        `Let @${agent.name} read the room?\n\n` +
        'Every message anyone types in this Circle will be sent to this agent\'s ' +
        'model provider — not just the ones addressed to it. Everyone in the Circle ' +
        'can see that this agent is listening.\n\n' +
        'It answers only when it has something to add, and files it writes while ' +
        'unaddressed are held for review.',
      )
      if (!ok) return
    }
    onChange({ engagement: ambient ? 'mention' : 'ambient' })
  }

  return (
    <div className="agent-engagement">
      <label title="Reply to this agent without re-typing its name; it only reads messages addressed to it.">
        <input
          type="checkbox"
          checked={ambient}
          disabled={busy}
          onChange={toggleAmbient}
        />
        <span>reads the room</span>
      </label>
      <label title={`Let another agent hand work to @${agent.name} by mentioning it. Chains are capped at ${agent.max_relay_turns} turns.`}>
        <input
          type="checkbox"
          checked={acceptsAgents}
          disabled={busy}
          onChange={() => onChange({ accept_from: acceptsAgents ? 'humans' : 'agents' })}
        />
        <span>accepts hand-offs</span>
      </label>
    </div>
  )
}
