import { useState, useEffect } from 'react'
import type { AgentConfigView } from '../types'
import { getAgentConfig } from '../api'

interface Props {
  onClose: () => void
}

/**
 * Read-only device settings. Surfaces this device's agent-reaction config
 * (~/.enoxian/agents.toml) so the operator can see how chat @mentions are
 * handled — without exposing the risky `push` toggle as a click. Editing stays
 * a deliberate file edit; see docs/plan/agent-workspaces.md → Two-Layer Split.
 */
export default function DeviceSettings({ onClose }: Props) {
  const [cfg, setCfg] = useState<AgentConfigView | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    getAgentConfig().then(setCfg).catch(e => setError(e.message))
  }, [])

  const isPush = cfg?.reaction === 'push'

  return (
    <div className="ritual-modal-backdrop" onClick={onClose}>
      <div className="ritual-panel sys-window" onClick={e => e.stopPropagation()} style={{ maxWidth: 460 }}>
        <button onClick={onClose} className="ritual-panel__close" aria-label="Close">×</button>
        <div className="ritual-panel__header">DEVICE SETTINGS</div>
        <div className="ritual-panel__body">
          <div className="ritual-divider" />

          <div className="group-label">AGENT MENTIONS</div>
          <p className="font-mono text-[10px] text-slate mb-3 leading-relaxed">
            How this device reacts when someone <code>@mentions</code> an agent in
            circle chat. A mention is only intent — this local policy decides
            whether it runs anything here.
          </p>

          {error && <div className="file-error">{error}</div>}
          {!cfg && !error && <div className="text-slate font-mono text-[11px]">Loading…</div>}

          {cfg && (
            <>
              {/* Reaction policy — shown, not toggled. */}
              <div className="flex items-center gap-2 mb-3 font-mono text-[11px]">
                <span className="text-[9px] font-bold text-slate">REACTION</span>
                <span
                  className={`text-[10px] font-bold px-2 py-0.5 border ${
                    isPush
                      ? 'border-obsidian bg-obsidian text-alabaster'
                      : 'border-obsidian text-obsidian'
                  }`}
                  title={isPush
                    ? 'A mention auto-runs the named agent on this device.'
                    : 'Mentions run nothing automatically; an agent must retrieve chat itself.'}
                >
                  {cfg.reaction.toUpperCase()}
                </span>
                <span className="text-[9px] text-slate">
                  {isPush ? 'mentions auto-run agents' : 'mentions do nothing here'}
                </span>
              </div>

              {isPush && (
                <div className="mb-3 border border-obsidian/40 px-2 py-1.5 font-mono text-[9px] text-slate leading-relaxed">
                  ⚠ PUSH is active — a circle member's mention can run one of the
                  agents below on this machine. To change this, edit the config
                  file (below).
                </div>
              )}

              {/* Configured agents (the allowlist). */}
              <div className="group-label">CONFIGURED AGENTS</div>
              {cfg.agents.length === 0 ? (
                <div className="text-slate font-mono text-[11px] mb-3">
                  {cfg.configured
                    ? 'No agents configured — mentions match nothing.'
                    : 'No agents.toml on this device — mentions match nothing.'}
                </div>
              ) : (
                <div className="flex flex-col gap-2 mb-3">
                  {cfg.agents.map(a => (
                    <div key={a.name} className="border border-obsidian/30 px-2 py-1.5 font-mono text-[11px]">
                      <div className="flex items-center justify-between gap-2">
                        <span className="font-bold">@{a.name}</span>
                        <span className="text-[9px] font-bold border border-obsidian/40 px-1 text-slate">
                          {a.driver.toUpperCase()}
                        </span>
                      </div>
                      <div className="text-[9px] text-slate mt-1 break-all">
                        {a.command.join(' ')}
                        {a.working_dir ? `  (in ${a.working_dir})` : ''}
                      </div>
                    </div>
                  ))}
                </div>
              )}

              {/* Where to edit — editing is file-only, on purpose. */}
              <div className="group-label">CONFIG FILE</div>
              <p className="font-mono text-[9px] text-slate leading-relaxed">
                Edit to change agents or the reaction policy — this is deliberately
                not editable here, since PUSH lets a mention run a local process.
              </p>
              <code className="block font-mono text-[10px] font-bold border border-obsidian px-2 py-1 mt-1 bg-white break-all">
                {cfg.config_path || '~/.enoxian/agents.toml'}
              </code>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
