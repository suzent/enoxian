import { useState, useEffect, useCallback } from 'react'
import { Bot, RadioTower, Laptop } from 'lucide-react'
import type { AgentConfigView, AgentPlugin, ConnectivitySettings, DiscoveredAgent } from '../types'
import type { IdentityInfo } from '../api'
import { getAgentConfigFor, getAgentPlugins, discoverAgents, installAgentPlugin, setEngagement, addAgent, removeAgent, getConnectivitySettings, setForceRelay, getIdentity, setIdentity } from '../api'
import { useApp } from '../context/AppContext'
import SegmentedTabs, { type SegmentedTabOption } from './ui/SegmentedTabs'
import EngagementSettings, { type Patch } from './EngagementSettings'
import DeviceIdentity from './DeviceIdentity'
import Select from './ui/Select'

type SettingsTab = 'device' | 'agents' | 'behaviour' | 'connectivity'

const SETTINGS_TABS: readonly SegmentedTabOption<SettingsTab>[] = [
  { value: 'device', content: <><Laptop size={14} aria-hidden="true" />DEVICE</> },
  { value: 'agents', content: <><Bot size={14} aria-hidden="true" />AGENTS</> },
  { value: 'behaviour', content: <><Bot size={14} aria-hidden="true" />BEHAVIOUR</> },
  { value: 'connectivity', content: <><RadioTower size={14} aria-hidden="true" />CONNECTIVITY</> },
]

interface Props {
  onClose: () => void
}

/**
 * Device settings — view and edit this device's agent config
 * (~/.enoxian/agents.toml) over the loopback API. Edits this machine's own
 * config only; never synced. Switching to `push` (which lets a chat mention run
 * a local process) is gated behind a confirm; adding/removing agents is
 * ordinary launcher config. See docs/concepts/proposals.md.
 */
export default function DeviceSettings({ onClose }: Props) {
  const { activeCircleId: chatCircleId, circles } = useApp()
  // Settings selection is independent of the active chat.
  const [activeCircleId, setSettingsCircleId] = useState(chatCircleId)
  const [activeTab, setActiveTab] = useState<SettingsTab>('behaviour')
  const [cfg, setCfg] = useState<AgentConfigView | null>(null)
  const [plugins, setPlugins] = useState<AgentPlugin[] | null>(null)
  const [known, setKnown] = useState<DiscoveredAgent[]>([])
  const [connectivity, setConnectivity] = useState<ConnectivitySettings | null>(null)
  const [identity, setIdentityState] = useState<IdentityInfo | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [installingPlugin, setInstallingPlugin] = useState<string | null>(null)

  // Add-agent form state.
  const [showAdd, setShowAdd] = useState(false)
  const [name, setName] = useState('')
  const [driver, setDriver] = useState<'acp' | 'argv'>('acp')
  const [command, setCommand] = useState('')

  const refresh = useCallback(() => {
    setError(null)
    setCfg(null)
    setPlugins(null)
    getAgentConfigFor(activeCircleId).then(setCfg).catch(e => setError(e.message))
    getAgentPlugins().then(r => setPlugins(r.plugins)).catch(() => setPlugins([]))
    // Descriptions for agents enoxian knows about, so a custom entry that is
    // one of them reads like an adapter instead of a bare command line.
    discoverAgents().then(r => setKnown(r.agents)).catch(() => setKnown([]))
    getIdentity().then(setIdentityState).catch(() => setIdentityState(null))
  }, [activeCircleId])

  useEffect(() => { refresh() }, [refresh])

  useEffect(() => {
    if (activeTab !== 'connectivity' || !activeCircleId) return
    setConnectivity(null)
    setError(null)
    getConnectivitySettings(activeCircleId)
      .then(setConnectivity)
      .catch(e => setError(e.message))
  }, [activeTab, activeCircleId])


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

  // Which scope the engagement controls edit. Global is the default because
  // most people have one answer for every Circle; the tab is there for when
  // they do not.
  const [scope, setScope] = useState<'global' | 'circle'>('global')
  const applyEngagement = (patch: Patch) =>
    run(() => setEngagement({
      ...(scope === 'circle' && activeCircleId ? { circle_id: activeCircleId } : {}),
      ...patch,
    }))

  const managedNames = new Set((plugins || []).map(plugin => plugin.agent))
  const customAgents = cfg?.agents.filter(agent => !managedNames.has(agent.name)) || []
  const activeCircle = circles.find(circle => circle.circle_id === activeCircleId)

  return (
    <div className="ritual-modal-backdrop" onClick={onClose}>
      <div className="ritual-panel sys-window device-settings-panel" onClick={e => e.stopPropagation()}>
        <button onClick={onClose} className="ritual-panel__close" aria-label="Close">×</button>
        <div className="ritual-panel__header">SETTINGS</div>
        <div className="settings-layout">
          <aside className="settings-sidebar">
                  <label className="settings-scope-picker">
                    <span>Settings for</span>
                    <Select
                      aria-label="Settings scope"
                      value={scope === 'global' ? '' : activeCircleId ?? ''}
                      disabled={busy}
                      onChange={id => {
                        setScope(id ? 'circle' : 'global')
                        if ((id && (activeTab === 'device' || activeTab === 'agents')) || (!id && activeTab === 'connectivity')) setActiveTab('behaviour')
                        if (id) setSettingsCircleId(id)
                      }}
                      options={[
                        { value: '', label: 'Global settings' },
                        ...circles.map(circle => ({ value: circle.circle_id, label: circle.circle_name, group: 'Circles' })),
                      ]}
                    />
                  </label>
          <SegmentedTabs
            className="settings-tabs"
            ariaLabel="Settings sections"
            orientation="vertical"
            value={activeTab}
            onChange={setActiveTab}
            options={SETTINGS_TABS.filter(tab => scope === 'global' ? tab.value !== 'connectivity' : tab.value === 'behaviour' || tab.value === 'connectivity')}
          />
          <p className="settings-sidebar__note">{scope === 'global' ? 'Defaults for this device across all Circles.' : 'This device’s preferences in the selected Circle.'}</p>
          </aside>
          <div className="ritual-panel__body settings-panel-body flex flex-col gap-4">
          <header className="settings-section-heading">
            <span className="settings-section-heading__scope">{scope === 'global' ? 'THIS DEVICE · GLOBAL' : `THIS DEVICE IN ${activeCircle?.circle_name ?? 'CIRCLE'}`}</span>
            <h2>{{ device: 'Device identity', agents: 'Your agents', behaviour: 'Agent behaviour', connectivity: 'Connectivity' }[activeTab]}</h2>
            <p>{{ device: 'How you and this machine appear to others.', agents: 'Connect the agents available on this machine. Used across all Circles.', behaviour: 'Set defaults for all Circles, or tailor how agents respond in one Circle.', connectivity: 'Manage how this device connects to the current Circle.' }[activeTab]}</p>
          </header>
          {error && (
            <div className="file-error">
              <div>{error}</div>
              <button type="button" className="enox-btn mt-2 text-[9px] px-2 py-1 min-h-0" onClick={refresh}>
                TRY AGAIN
              </button>
            </div>
          )}
          {activeTab === 'device' && !identity && !error && (
            <div className="text-slate font-mono text-[11px]">Loading…</div>
          )}

          {activeTab === 'device' && identity && (
            <>
              <section>
                <div className="text-[11px] font-bold mb-1">IDENTITY</div>
                <DeviceIdentity
                  identity={identity}
                  addressedAs={cfg?.circle?.addressed_as}
                  circleName={activeCircle?.circle_name}
                  busy={busy}
                  onSave={async patch => {
                    await run(() => setIdentity(patch))
                    await getIdentity().then(setIdentityState).catch(() => {})
                  }}
                />
              </section>

              {cfg?.config_path && (
                <section>
                  <div className="text-[11px] font-bold mb-1">CONFIG</div>
                  <div className="text-[9px] text-slate break-all">{cfg.config_path}</div>
                  <div className="text-[9px] text-slate mt-1 leading-relaxed">
                    Agent configuration is stored here on this machine, including your Circle overrides.
                  </div>
                </section>
              )}
            </>
          )}

          {(activeTab === 'agents' || activeTab === 'behaviour') && !cfg && !error && <div className="text-slate font-mono text-[11px]">Loading…</div>}

          {activeTab === 'behaviour' && cfg && (
            <>
                <section className="settings-scope">
                <div className="settings-scope__note">
                  {scope === 'global'
                    ? 'Applies in every Circle, unless one of them overrides it.'
                    : activeCircleId
                      ? <>Applies in <strong>{activeCircle?.circle_name}</strong> only. Anything left inherited follows the settings for all Circles.</>
                      : 'Open a Circle to give it its own settings.'}
                  {' '}
                  {/* A per-Circle setting reads like it belongs to the Circle.
                      It does not: it is this machine's answer about that
                      Circle, and nobody else can see or change it. Saying so
                      here is cheaper than the misunderstanding. */}
                  <span className="settings-scope__private">
                    These are this device's settings — never shared with the Circle.
                  </span>
                </div>
                {scope === 'global' && <label className="block py-2 text-xs">
                  Maximum concurrent agents on this device
                  <select className="ml-2 border px-2 py-1" disabled={busy} value={cfg.max_concurrent_runs ?? 4}
                    onChange={e => void run(() => setEngagement({ max_concurrent_runs: Number(e.target.value) }))}>
                    {[1, 2, 4, 8, 16, 32].map(n => <option key={n} value={n}>{n === 1 ? '1 (serial)' : n}</option>)}
                  </select>
                  <span className="block text-slate">Takes effect after restarting the daemon. Queued requests are retained.</span>
                </label>}
                {scope === 'circle' && !activeCircleId ? null : (
                  <EngagementSettings
                    agentNames={cfg.agents.map(a => a.name)}
                    global={cfg.global_settings}
                    circle={cfg.circle}
                    scope={scope}
                    busy={busy}
                    onChange={applyEngagement}
                  />
                )}
              </section>

            </>
          )}

          {activeTab === 'agents' && cfg && (
            <>
              {plugins && plugins.length > 0 && (
                <section>
                  <div className="flex items-baseline justify-between border-b border-obsidian pb-1 mb-1">
                    <span className="font-mono text-[11px] font-bold">AGENT ADAPTERS</span>
                    {/* Only npm adapters are pinned; a native plugin is whatever
                        CLI the user installed, so do not claim otherwise. */}
                    <span className="font-mono text-[8px] text-slate">
                      {plugins.every(p => p.kind === 'npm') ? 'LOCAL · PINNED' : 'LOCAL'}
                    </span>
                  </div>
                  <div className="divide-y divide-obsidian/20">
                    {plugins.map(plugin => {
                      const native = plugin.kind === 'native'
                      const runtimeMissing = plugin.runtime_installed === false
                      // A native plugin never uses Node, so it must not be
                      // gated on a runtime it does not have.
                      const nodeMissing = plugin.requires_node && !plugin.node_runtime_installed
                      const prerequisitesMissing = runtimeMissing || nodeMissing
                      const ready = plugin.state === 'ready' && plugin.configured && !prerequisitesMissing
                      const installing = installingPlugin === plugin.id || plugin.state === 'installing'
                      // Nothing is downloaded for a native plugin: enabling it
                      // only writes the chat handle, so never say "install".
                      const action = native
                        ? 'ENABLE'
                        : plugin.state === 'broken'
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
                            ? native ? 'Installed · not enabled' : 'Installed · disabled'
                            : plugin.state === 'broken' ? 'Needs repair' : 'Not installed'
                      return (
                      <div key={plugin.id} className="py-2 font-mono">
                        <div className="flex items-center justify-between gap-3">
                          <div className="min-w-0">
                            <div className="flex items-center gap-2">
                              <span className="text-[11px] font-bold">@{plugin.agent}</span>
                              <span className="text-[8px] text-slate">{plugin.version ? `v${plugin.version}` : 'NATIVE ACP'}</span>
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
                                title={native
                                  ? `Point @${plugin.agent} at your installed ${plugin.runtime_program || 'CLI'} — nothing is downloaded`
                                  : `Install ${plugin.package}@${plugin.version}`}
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
                            {native ? (
                              <>
                                Install <code>{plugin.runtime_program}</code>
                                {plugin.install_url ? <> from <a href={plugin.install_url} target="_blank" rel="noreferrer" className="underline text-obsidian">{new URL(plugin.install_url).host}</a></> : ''}
                                , then check again. Enoxian downloads nothing for this agent.
                              </>
                            ) : (
                              <>
                                Install the official {plugin.runtime_program || 'product'} CLI and authenticate it
                                {plugin.runtime_login_command ? <> with <code>{plugin.runtime_login_command}</code></> : ''}.
                              </>
                            )}
                          </div>
                        )}

                        {ready && plugin.runtime_program && (
                          <div className="mt-2 border-l-2 border-obsidian/40 pl-2 text-[9px] text-slate leading-relaxed">
                            {native ? (
                              <>
                                Runs your installed <code>{plugin.runtime_program}</code> CLI, which speaks ACP itself —
                                Enoxian pins nothing and downloads nothing. Its own memory, skills, and settings stay yours.
                              </>
                            ) : (
                              <>
                                Runs your installed <code>{plugin.runtime_program}</code> CLI. Enoxian manages only the
                                adapter, so that CLI's login and settings stay yours.
                              </>
                            )}
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
                        onKeyDown={e => {
                          // Enter during an IME composition accepts a
                          // candidate; it must not submit the form.
                          const native = e.nativeEvent as KeyboardEvent
                          if (native.isComposing || native.keyCode === 229) return
                          if (e.key === 'Enter') submitAdd()
                        }}
                        placeholder="executable and arguments"
                        className="border border-obsidian px-2 py-1 text-[11px] focus:outline-none focus:bg-obsidian/5"
                      />
                      <button onClick={submitAdd} disabled={busy} className="enox-btn self-start text-[9px] px-2 py-1 min-h-0 disabled:opacity-50">ADD</button>
                    </div>
                  )}

                  {customAgents.map(agent => {
                    const about = known.find(k => k.name === agent.name)?.about
                    // The backend already probes command[0]; a configured agent
                    // whose program is gone would fail at launch, so say so here
                    // rather than at mention time.
                    const health = agent.status === 'ready'
                      ? { label: 'READY', ready: true, detail: about }
                      : agent.status === 'runtime_download'
                        ? { label: 'DOWNLOADS', ready: false, detail: 'Runs a package manager on first use · migrate to a pinned adapter' }
                        : { label: 'MISSING', ready: false, detail: `${agent.command[0] || 'Command'} not found on PATH` }
                    return (
                    <div key={agent.name} className="flex items-start justify-between gap-2 border-b border-obsidian/20 pb-2 text-[10px]">
                      <div className="min-w-0">
                        <div className="font-bold">@{agent.name} <span className="text-[8px] text-slate">{agent.driver.toUpperCase()}</span></div>
                        {health.detail && (
                          <div className={`text-[9px] mt-0.5 ${health.ready ? 'text-obsidian' : 'text-slate'}`}>{health.detail}</div>
                        )}
                        <div className="text-[8px] text-slate truncate" title={agent.command.join(' ')}>{agent.command.join(' ')}</div>
                      </div>
                      <div className="flex items-center gap-2 shrink-0">
                        <span
                          className={`text-[8px] font-bold px-1.5 py-0.5 ${health.ready ? 'bg-obsidian text-alabaster' : 'border border-obsidian/40 text-slate'}`}
                          title={health.ready ? 'command[0] resolves on this machine' : 'This agent cannot start until its command resolves'}
                        >{health.label}</span>
                        <button onClick={() => run(() => removeAgent(agent.name))} disabled={busy} className="text-slate hover:text-obsidian">×</button>
                      </div>
                    </div>
                    )
                  })}
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

          {activeTab === 'connectivity' && !activeCircleId && (
            <div className="settings-empty">Open a Circle to view its connection settings.</div>
          )}

          {activeTab === 'connectivity' && activeCircleId && !connectivity && !error && (
            <div className="text-slate font-mono text-[11px]">Loading…</div>
          )}

          {activeTab === 'connectivity' && activeCircleId && connectivity && (
            <div className="settings-connectivity">
              <div className="settings-connectivity__circle">
                <span>CURRENT CIRCLE</span>
                <strong>{activeCircle?.circle_name ?? activeCircleId ?? 'NONE'}</strong>
              </div>
              {/* Routing is per Circle and stored on this device, the same
                  shape as the per-Circle engagement overrides. Saying so keeps
                  the two consistent — and stops "force relay" reading like
                  something the whole Circle is switched to. */}
              <div className="settings-scope__note">
                Applies in <strong>{activeCircle?.circle_name ?? 'this Circle'}</strong> only.{' '}
                <span className="settings-scope__private">
                  This is this device's routing — never shared with the Circle.
                </span>
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
