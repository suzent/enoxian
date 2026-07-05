import { useState, useEffect, useCallback } from 'react'
import type { AgentConfigView, DiscoveredAgent } from '../types'
import { getAgentConfig, discoverAgents, setAgentReaction, addAgent, removeAgent } from '../api'

interface Props {
  onClose: () => void
}

/**
 * Device settings — view and edit this device's agent config
 * (~/.enoxian/agents.toml) over the loopback API. Edits this machine's own
 * config only; never synced. Switching to `push` (which lets a chat mention run
 * a local process) is gated behind a confirm; adding/removing agents is
 * ordinary launcher config. See docs/plan/agent-workspaces.md → Two-Layer Split.
 */
export default function DeviceSettings({ onClose }: Props) {
  const [cfg, setCfg] = useState<AgentConfigView | null>(null)
  const [discovered, setDiscovered] = useState<DiscoveredAgent[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  // Add-agent form state.
  const [showAdd, setShowAdd] = useState(false)
  const [name, setName] = useState('')
  const [driver, setDriver] = useState<'acp' | 'argv'>('acp')
  const [command, setCommand] = useState('')

  const refresh = useCallback(() => {
    getAgentConfig().then(setCfg).catch(e => setError(e.message))
    // Discovery is best-effort — a failure here shouldn't block config editing.
    discoverAgents().then(r => setDiscovered(r.agents)).catch(() => setDiscovered([]))
  }, [])

  useEffect(() => { refresh() }, [refresh])

  const isPush = cfg?.reaction === 'push'

  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true)
    setError(null)
    try {
      await fn()
      refresh()
    } catch (e: any) {
      setError(e.message)
    } finally {
      setBusy(false)
    }
  }

  const toggleReaction = () => {
    if (!cfg) return
    if (!isPush) {
      // Arming push is the sensitive action — confirm before enabling.
      const ok = window.confirm(
        'Enable PUSH?\n\nWith push on, any circle member who @mentions one of your ' +
        'configured agents can run it as a process on THIS machine. Only enable if you ' +
        'trust the circle and the agents below.',
      )
      if (!ok) return
    }
    run(() => setAgentReaction(isPush ? 'pull' : 'push'))
  }

  const submitAdd = () => {
    const parts = command.trim().split(/\s+/).filter(Boolean)
    if (!name.trim() || parts.length === 0) return
    run(() => addAgent(name.trim(), driver, parts)).then(() => {
      setName(''); setCommand(''); setDriver('acp'); setShowAdd(false)
    })
  }

  return (
    <div className="ritual-modal-backdrop" onClick={onClose}>
      <div className="ritual-panel sys-window" onClick={e => e.stopPropagation()} style={{ maxWidth: 480 }}>
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
              {/* Reaction policy — clickable toggle, confirm before arming push. */}
              <div className="flex items-center gap-2 mb-3 font-mono text-[11px]">
                <span className="text-[9px] font-bold text-slate">REACTION</span>
                <button
                  onClick={toggleReaction}
                  disabled={busy}
                  className={`text-[10px] font-bold px-2 py-0.5 border cursor-pointer disabled:opacity-50 ${
                    isPush
                      ? 'border-obsidian bg-obsidian text-alabaster'
                      : 'border-obsidian text-obsidian hover:bg-obsidian/10'
                  }`}
                  title="Click to toggle push/pull"
                >
                  {cfg.reaction.toUpperCase()}
                </button>
                <span className="text-[9px] text-slate">
                  {isPush ? 'mentions auto-run agents · click to disable' : 'mentions do nothing · click to enable'}
                </span>
              </div>

              {isPush && (
                <div className="mb-3 border border-obsidian/40 px-2 py-1.5 font-mono text-[9px] text-slate leading-relaxed">
                  ⚠ PUSH is active — a circle member's mention can run one of the
                  agents below on this machine.
                </div>
              )}

              {/* Discovered agents — well-known candidates the daemon probed
                  for on this machine's PATH. Only installed, not-yet-configured
                  ones offer a one-click add. */}
              {discovered && discovered.some(d => d.installed && !d.configured) && (
                <>
                  <div className="group-label">DETECTED ON THIS MACHINE</div>
                  <p className="font-mono text-[9px] text-slate mb-2 leading-relaxed">
                    Agents found on your <code>PATH</code>. Adding one writes it to
                    the config below — nothing runs until a mention triggers it.
                  </p>
                  <div className="flex flex-col gap-2 mb-3">
                    {discovered.filter(d => d.installed && !d.configured).map(d => (
                      <div key={d.name} className="border border-dashed border-obsidian/40 px-2 py-1.5 font-mono text-[11px]">
                        <div className="flex items-center justify-between gap-2">
                          <span className="font-bold">@{d.name}</span>
                          <div className="flex items-center gap-2 shrink-0">
                            <span className="text-[9px] font-bold border border-obsidian/40 px-1 text-slate">
                              {d.driver.toUpperCase()}
                            </span>
                            <button
                              onClick={() => run(() => addAgent(d.name, d.driver, d.command))}
                              disabled={busy}
                              className="enox-btn text-[9px] px-1.5 py-0.5 disabled:opacity-50"
                              title={`Add @${d.name}`}
                            >+ ADD</button>
                          </div>
                        </div>
                        <div className="text-[9px] text-slate mt-1 leading-relaxed">{d.about}</div>
                        <div className="text-[9px] text-slate mt-0.5 break-all opacity-70">{d.command.join(' ')}</div>
                      </div>
                    ))}
                  </div>
                </>
              )}

              {/* Configured agents — each removable. */}
              <div className="group-label flex items-center justify-between">
                <span>CONFIGURED AGENTS</span>
                <button
                  onClick={() => setShowAdd(v => !v)}
                  className="text-[10px] font-bold px-1 border border-obsidian hover:bg-obsidian hover:text-alabaster"
                  title={showAdd ? 'Cancel' : 'Add an agent'}
                >{showAdd ? '×' : '+'}</button>
              </div>

              {showAdd && (
                <div className="border border-dashed border-obsidian/50 p-2 mb-3 flex flex-col gap-2 font-mono text-[11px]">
                  <input
                    autoFocus value={name} onChange={e => setName(e.target.value)}
                    placeholder="name (e.g. claude)"
                    className="border border-obsidian px-2 py-1 text-[11px] focus:outline-none focus:bg-obsidian/5"
                  />
                  <div className="flex gap-2 items-center">
                    <span className="text-[9px] text-slate">DRIVER</span>
                    {(['acp', 'argv'] as const).map(d => (
                      <button key={d} onClick={() => setDriver(d)}
                        className={`text-[9px] font-bold px-2 py-0.5 border ${driver === d ? 'bg-obsidian text-alabaster border-obsidian' : 'border-obsidian/40 text-slate'}`}
                      >{d.toUpperCase()}</button>
                    ))}
                  </div>
                  <input
                    value={command} onChange={e => setCommand(e.target.value)}
                    onKeyDown={e => e.key === 'Enter' && submitAdd()}
                    placeholder="command, e.g. npx @zed-industries/claude-code-acp"
                    className="border border-obsidian px-2 py-1 text-[11px] focus:outline-none focus:bg-obsidian/5"
                  />
                  <button onClick={submitAdd} disabled={busy} className="enox-btn self-start disabled:opacity-50">ADD</button>
                </div>
              )}

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
                        <div className="flex items-center gap-2 shrink-0">
                          {!a.installed && (
                            <span
                              className="text-[9px] font-bold border border-obsidian px-1 bg-obsidian text-alabaster"
                              title={`${a.command[0]} was not found on PATH — this agent would fail to launch`}
                            >MISSING</span>
                          )}
                          <span className="text-[9px] font-bold border border-obsidian/40 px-1 text-slate">
                            {a.driver.toUpperCase()}
                          </span>
                          <button
                            onClick={() => run(() => removeAgent(a.name))}
                            disabled={busy}
                            className="text-[9px] text-slate hover:text-obsidian font-bold px-1 disabled:opacity-50"
                            title={`Remove @${a.name}`}
                          >×</button>
                        </div>
                      </div>
                      <div className="text-[9px] text-slate mt-1 break-all">
                        {a.command.join(' ')}
                        {a.working_dir ? `  (in ${a.working_dir})` : ''}
                      </div>
                    </div>
                  ))}
                </div>
              )}

              {/* The underlying file — still hand-editable; edits here rewrite it. */}
              <div className="group-label">CONFIG FILE</div>
              <p className="font-mono text-[9px] text-slate leading-relaxed">
                Edits here rewrite this file (comments are not preserved). You can
                also edit it by hand.
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
