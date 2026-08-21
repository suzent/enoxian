import { useState, useEffect, useCallback } from 'react'
import { Bot, RadioTower } from 'lucide-react'
import type { AgentConfigView, AgentPlugin, ConnectivitySettings } from '../types'
import { getAgentConfig, getAgentPlugins, installAgentPlugin, setAgentReaction, addAgent, removeAgent, getConnectivitySettings, setForceRelay } from '../api'
import { useApp } from '../context/AppContext'

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
  const { activeCircleId, circles } = useApp()
  const [activeTab, setActiveTab] = useState<'agents' | 'connectivity'>('agents')
  const [cfg, setCfg] = useState<AgentConfigView | null>(null)
  const [plugins, setPlugins] = useState<AgentPlugin[] | null>(null)
  const [connectivity, setConnectivity] = useState<ConnectivitySettings | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [installingPlugin, setInstallingPlugin] = useState<string | null>(null)

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

  useEffect(() => {
    if (activeTab !== 'connectivity' || !activeCircleId) return
    setConnectivity(null)
    setError(null)
    getConnectivitySettings(activeCircleId)
      .then(setConnectivity)
      .catch(e => setError(e.message))
  }, [activeTab, activeCircleId])

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

  const toggleForceRelay = async () => {
    if (!activeCircleId || !connectivity || busy) return
    setBusy(true)
    setError(null)
    try {
      const next = await setForceRelay(activeCircleId, !connectivity.force_relay)
      setConnectivity({ ...connectivity, ...next })
    } catch (e: any) {
      setError(e.message)
      getConnectivitySettings(activeCircleId).then(setConnectivity).catch(() => {})
    } finally {
      setBusy(false)
    }
  }

  const installPlugin = async (plugin: AgentPlugin) => {
    setInstallingPlugin(plugin.id)
    try {
      await run(() => installAgentPlugin(plugin.id))
    } finally {
      setInstallingPlugin(null)
    }
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
  const activeCircle = circles.find(circle => circle.circle_id === activeCircleId)

  return (
    <div className="ritual-modal-backdrop" onClick={onClose}>
      <div className="ritual-panel sys-window device-settings-panel" onClick={e => e.stopPropagation()}>
        <button onClick={onClose} className="ritual-panel__close" aria-label="Close">×</button>
        <div className="ritual-panel__header">DEVICE SETTINGS</div>
        <div className="settings-layout">
          <div className="settings-tabs" role="tablist" aria-label="Settings sections" aria-orientation="vertical">
            <button
              type="button"
              role="tab"
              aria-selected={activeTab === 'agents'}
              className={activeTab === 'agents' ? 'is-active' : ''}
              onClick={() => setActiveTab('agents')}
            >
              <Bot size={14} aria-hidden="true" />
              AGENTS
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={activeTab === 'connectivity'}
              className={activeTab === 'connectivity' ? 'is-active' : ''}
              onClick={() => setActiveTab('connectivity')}
            >
              <RadioTower size={14} aria-hidden="true" />
              CONNECTIVITY
            </button>
          </div>
          <div className="ritual-panel__body settings-panel-body flex flex-col gap-4">
          {error && <div className="file-error">{error}</div>}
          {activeTab === 'agents' && !cfg && !error && <div className="text-slate font-mono text-[11px]">Loading…</div>}

          {activeTab === 'agents' && cfg && (
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
                      const runtimeMissing = plugin.runtime_installed === false
                      const nodeMissing = !plugin.node_runtime_installed
                      const prerequisitesMissing = runtimeMissing || nodeMissing
                      const ready = plugin.state === 'ready' && plugin.configured && !prerequisitesMissing
                      const installing = installingPlugin === plugin.id || plugin.state === 'installing'
                      const action = plugin.state === 'broken'
                        ? 'REPAIR'
                        : plugin.state === 'ready'
                          ? 'USE MANAGED'
                          : installing ? 'PREPARING…' : 'INSTALL'
                      const status = runtimeMissing
                        ? `${plugin.runtime_program || 'Product'} CLI missing`
                        : nodeMissing
                          ? plugin.node_runtime_version
                            ? `Node.js ${plugin.node_runtime_version} is too old · requires 22+ with npm`
                            : 'Node.js 22+ with npm required'
                        : ready
                        ? 'Ready'
                        : plugin.legacy_configured
                          ? 'Runtime download · migrate'
                          : plugin.state === 'ready'
                            ? 'Installed · disabled'
                            : plugin.state === 'broken' ? 'Needs repair' : 'Not installed'
                      return (
                      <div key={plugin.id} className="py-2 font-mono">
                        <div className="flex items-center justify-between gap-3">
                          <div className="min-w-0">
                            <div className="flex items-center gap-2">
                              <span className="text-[11px] font-bold">@{plugin.agent}</span>
                              <span className="text-[8px] text-slate">v{plugin.version}</span>
                            </div>
                            <div className={`text-[9px] mt-0.5 ${ready ? 'text-obsidian' : 'text-slate'}`}>
                              {installing ? 'Preparing runtime and pinned adapter…' : status}
                            </div>
                          </div>
                          <div className="flex items-center justify-between gap-2">
                            {ready ? (
                              <span className="text-[9px] font-bold px-1.5 py-0.5 bg-obsidian text-alabaster">READY</span>
                            ) : prerequisitesMissing ? (
                              <button
                                onClick={() => { setError(null); refresh() }}
                                disabled={busy}
                                className="enox-btn text-[9px] px-2 py-1 min-h-0 disabled:opacity-50"
                                title="Install the missing prerequisite, restart Enoxian, then check again"
                              >CHECK AGAIN</button>
                            ) : (
                              <button
                                onClick={() => installPlugin(plugin)}
                                disabled={busy || installing}
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

                        {runtimeMissing && (
                          <div className="mt-2 border-l-2 border-obsidian/40 pl-2 text-[9px] text-slate leading-relaxed">
                            Install the official {plugin.runtime_program || 'product'} CLI and authenticate it
                            {plugin.runtime_login_command ? <> with <code>{plugin.runtime_login_command}</code></> : ''}.
                          </div>
                        )}

                        {nodeMissing && (
                          <div className="mt-2 border-l-2 border-obsidian/40 pl-2 text-[9px] text-slate leading-relaxed">
                            Install system Node.js 22+ with npm from{' '}
                            <a href="https://nodejs.org/en/download" target="_blank" rel="noreferrer" className="underline text-obsidian">nodejs.org</a>,
                            {' '}restart the Enoxian service, then check again. Enoxian does not install or manage Node.js.
                          </div>
                        )}

                        {installing && (
                          <div className="mt-2 h-1 overflow-hidden bg-obsidian/10" role="progressbar" aria-label={`Installing @${plugin.agent}`}>
                            <div className="h-full w-2/3 bg-obsidian animate-pulse" />
                          </div>
                        )}
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

          {activeTab === 'connectivity' && !connectivity && !error && (
            <div className="text-slate font-mono text-[11px]">Loading…</div>
          )}

          {activeTab === 'connectivity' && connectivity && (
            <div className="settings-connectivity">
              <div className="settings-connectivity__circle">
                <span>CURRENT CIRCLE</span>
                <strong>{activeCircle?.circle_name ?? activeCircleId ?? 'NONE'}</strong>
              </div>

              <div className="settings-connectivity__availability" aria-label="Connectivity services">
                <span className={connectivity.relay_configured ? 'is-ready' : ''}>
                  <i aria-hidden="true" /> RELAY
                </span>
                <span className={connectivity.rendezvous_configured ? 'is-ready' : ''}>
                  <i aria-hidden="true" /> RENDEZVOUS
                </span>
              </div>

              <section className="settings-connectivity__mode">
                <div>
                  <div className="settings-connectivity__title">
                    FORCE RELAY
                    <span>DIAGNOSTIC</span>
                  </div>
                  <div className="settings-connectivity__status">
                    {busy ? 'RESTARTING CIRCLE…' : connectivity.force_relay ? 'RELAY ONLY' : 'AUTOMATIC ROUTING'}
                  </div>
                </div>
                <button
                  type="button"
                  role="switch"
                  aria-checked={connectivity.force_relay}
                  aria-label="Force relay"
                  disabled={busy || !activeCircleId}
                  className={`settings-switch${connectivity.force_relay ? ' is-on' : ''}`}
                  onClick={toggleForceRelay}
                >
                  <span aria-hidden="true" />
                </button>
              </section>
            </div>
          )}
          </div>
        </div>
      </div>
    </div>
  )
}
