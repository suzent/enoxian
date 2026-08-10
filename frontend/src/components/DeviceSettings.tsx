import { useState, useEffect, useCallback } from 'react'
import type { AgentConfigView, AgentPlugin } from '../types'
import { getAgentConfig, getAgentPlugins, installAgentPlugin, setAgentReaction, addAgent, removeAgent } from '../api'

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
  const [plugins, setPlugins] = useState<AgentPlugin[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  // Add-agent form state.
  const [showAdd, setShowAdd] = useState(false)
  const [name, setName] = useState('')
  const [driver, setDriver] = useState<'acp' | 'argv'>('acp')
  const [command, setCommand] = useState('')

  const refresh = useCallback(() => {
    getAgentConfig().then(setCfg).catch(e => setError(e.message))
    getAgentPlugins().then(r => setPlugins(r.plugins)).catch(() => setPlugins([]))
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

  const managedNames = new Set((plugins || []).map(plugin => plugin.agent))
  const customAgents = cfg?.agents.filter(agent => !managedNames.has(agent.name)) || []

  return (
    <div className="ritual-modal-backdrop" onClick={onClose}>
      <div className="ritual-panel sys-window" onClick={e => e.stopPropagation()} style={{ maxWidth: 440 }}>
        <button onClick={onClose} className="ritual-panel__close" aria-label="Close">×</button>
        <div className="ritual-panel__header">DEVICE SETTINGS</div>
        <div className="ritual-panel__body flex flex-col gap-4">
          {error && <div className="file-error">{error}</div>}
          {!cfg && !error && <div className="text-slate font-mono text-[11px]">Loading…</div>}

          {cfg && (
            <>
              <section className="flex items-center justify-between gap-4 border-b border-obsidian pb-3">
                <div className="min-w-0">
                  <div className="font-mono text-[11px] font-bold">MENTION AUTOMATION</div>
                  <div className="font-mono text-[9px] text-slate mt-0.5">
                    {isPush ? 'Agents run when mentioned in chat.' : 'Mentions never start local agents.'}
                  </div>
                </div>
                <button
                  onClick={toggleReaction}
                  disabled={busy}
                  className={`shrink-0 min-w-[54px] text-[10px] font-bold px-2 py-1 border cursor-pointer disabled:opacity-50 ${
                    isPush
                      ? 'border-obsidian bg-obsidian text-alabaster'
                      : 'border-obsidian text-obsidian hover:bg-obsidian/10'
                  }`}
                  title={isPush ? 'Disable mention automation' : 'Enable mention automation'}
                >
                  {isPush ? 'ON' : 'OFF'}
                </button>
              </section>

              {plugins && plugins.length > 0 && (
                <section>
                  <div className="flex items-baseline justify-between border-b border-obsidian pb-1 mb-1">
                    <span className="font-mono text-[11px] font-bold">AGENT ADAPTERS</span>
                    <span className="font-mono text-[8px] text-slate">LOCAL · PINNED</span>
                  </div>
                  <div className="divide-y divide-obsidian/20">
                    {plugins.map(plugin => {
                      const ready = plugin.state === 'ready' && plugin.configured
                      const action = plugin.state === 'broken'
                        ? 'REPAIR'
                        : plugin.state === 'ready'
                          ? 'USE MANAGED'
                          : plugin.state === 'installing' ? 'INSTALLING' : 'INSTALL'
                      const status = ready
                        ? 'Ready'
                        : plugin.legacy_configured
                          ? 'Runtime download · migrate'
                          : plugin.state === 'ready'
                            ? 'Installed · disabled'
                            : plugin.state === 'broken' ? 'Needs repair' : 'Not installed'
                      return (
                      <div key={plugin.id} className="flex items-center justify-between gap-3 py-2 font-mono">
                        <div className="min-w-0">
                          <div className="flex items-center gap-2">
                            <span className="text-[11px] font-bold">@{plugin.agent}</span>
                            <span className="text-[8px] text-slate">v{plugin.version}</span>
                          </div>
                          <div className={`text-[9px] mt-0.5 ${ready ? 'text-obsidian' : 'text-slate'}`}>
                            {status}
                          </div>
                        </div>
                        <div className="flex items-center justify-between gap-2">
                          {ready ? (
                            <span className="text-[9px] font-bold px-1.5 py-0.5 bg-obsidian text-alabaster">READY</span>
                          ) : (
                            <button
                              onClick={() => run(() => installAgentPlugin(plugin.id))}
                              disabled={busy || plugin.state === 'installing'}
                              className="enox-btn text-[9px] px-2 py-1 min-h-0 disabled:opacity-50"
                              title={`Install ${plugin.package}@${plugin.version}`}
                            >{action}</button>
                          )}
                          {(plugin.configured || plugin.legacy_configured) && (
                            <button
                              onClick={() => run(() => removeAgent(plugin.agent))}
                              disabled={busy}
                              className="text-[12px] text-slate hover:text-obsidian px-1 disabled:opacity-50"
                              title={`Disable @${plugin.agent}`}
                              aria-label={`Disable @${plugin.agent}`}
                            >×</button>
                          )}
                        </div>
                      </div>
                    )})}
                  </div>
                </section>
              )}

              <details className="font-mono border-t border-obsidian/30 pt-2">
                <summary className="cursor-pointer text-[10px] font-bold select-none">ADVANCED</summary>
                <div className="mt-3 flex flex-col gap-3">
                  <div className="flex items-center justify-between">
                    <span className="text-[9px] font-bold text-slate">CUSTOM AGENTS</span>
                    <button
                      onClick={() => setShowAdd(v => !v)}
                      className="text-[10px] font-bold px-1 border border-obsidian hover:bg-obsidian hover:text-alabaster"
                      title={showAdd ? 'Cancel' : 'Add a custom agent'}
                    >{showAdd ? '×' : '+'}</button>
                  </div>

                  {showAdd && (
                    <div className="border border-dashed border-obsidian/50 p-2 flex flex-col gap-2 text-[11px]">
                      <input
                        autoFocus value={name} onChange={e => setName(e.target.value)}
                        placeholder="agent name"
                        className="border border-obsidian px-2 py-1 text-[11px] focus:outline-none focus:bg-obsidian/5"
                      />
                      <div className="flex gap-2 items-center">
                        {(['acp', 'argv'] as const).map(d => (
                          <button key={d} onClick={() => setDriver(d)}
                            className={`text-[9px] font-bold px-2 py-0.5 border ${driver === d ? 'bg-obsidian text-alabaster border-obsidian' : 'border-obsidian/40 text-slate'}`}
                          >{d.toUpperCase()}</button>
                        ))}
                      </div>
                      <input
                        value={command} onChange={e => setCommand(e.target.value)}
                        onKeyDown={e => e.key === 'Enter' && submitAdd()}
                        placeholder="executable and arguments"
                        className="border border-obsidian px-2 py-1 text-[11px] focus:outline-none focus:bg-obsidian/5"
                      />
                      <button onClick={submitAdd} disabled={busy} className="enox-btn self-start text-[9px] px-2 py-1 min-h-0 disabled:opacity-50">ADD</button>
                    </div>
                  )}

                  {customAgents.map(agent => (
                    <div key={agent.name} className="flex items-center justify-between gap-2 border-b border-obsidian/20 pb-2 text-[10px]">
                      <div className="min-w-0">
                        <div className="font-bold">@{agent.name} <span className="text-[8px] text-slate">{agent.driver.toUpperCase()}</span></div>
                        <div className="text-[8px] text-slate truncate" title={agent.command.join(' ')}>{agent.command.join(' ')}</div>
                      </div>
                      <button onClick={() => run(() => removeAgent(agent.name))} disabled={busy} className="text-slate hover:text-obsidian">×</button>
                    </div>
                  ))}
                  {customAgents.length === 0 && !showAdd && (
                    <div className="text-[9px] text-slate">No custom agents.</div>
                  )}

                  <div>
                    <div className="text-[9px] font-bold text-slate mb-1">CONFIG FILE</div>
                    <code className="block text-[9px] border border-obsidian/40 px-2 py-1 bg-white break-all normal-case">
                      {cfg.config_path || '~/.enoxian/agents.toml'}
                    </code>
                  </div>
                </div>
              </details>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
