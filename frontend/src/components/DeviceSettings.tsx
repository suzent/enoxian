import { useState, useEffect, useCallback, useRef, type ReactNode } from 'react'
import { Bot, RadioTower, Laptop, Users, SlidersHorizontal } from 'lucide-react'
import type { AgentConfigView, AgentPlugin, ConnectivitySettings, DiscoveredAgent } from '../types'
import type { IdentityInfo } from '../api'
import { getAgentConfigFor, getAgentPlugins, discoverAgents, installAgentPlugin, setEngagement, addAgent, removeAgent, getConnectivitySettings, setForceRelay, getIdentity, setIdentity } from '../api'
import { useApp } from '../context/AppContext'
import SegmentedTabs, { type SegmentedTabOption } from './ui/SegmentedTabs'
import EngagementSettings, { type Patch } from './EngagementSettings'
import DeviceIdentity from './DeviceIdentity'
import Dialog, { ConfirmDialog } from './ui/Dialog'
import Select from './ui/Select'
import AgentAuthentication, { useAgentAuthentication } from './AgentAuthentication'
import AgentCards from './AgentCards'
import WardenSettings from './WardenSettings'

type SettingsTab = 'device' | 'agents' | 'behaviour' | 'connectivity' | 'membership'

const SETTINGS_TABS: readonly SegmentedTabOption<SettingsTab>[] = [
  { value: 'device', content: <><Laptop size={14} aria-hidden="true" />Device</> },
  { value: 'agents', content: <><Bot size={14} aria-hidden="true" />Agents</> },
  { value: 'behaviour', content: <><SlidersHorizontal size={14} aria-hidden="true" />Behaviour</> },
  { value: 'membership', content: <><Users size={14} aria-hidden="true" />Membership</> },
  { value: 'connectivity', content: <><RadioTower size={14} aria-hidden="true" />Connectivity</> },
]

/** `relay.enoxian.com` for `https://relay.enoxian.com/`. */
function relayHost(url: string): string {
  try {
    return new URL(url).host.replace(/\.$/, '')
  } catch {
    return url
  }
}

interface Props {
  onClose: () => void
  circleId?: string
  membership?: ReactNode
  /** Open a shared-folder file in the editor, closing settings. */
  onOpenFile?: (path: string) => void
}

/**
 * Device settings — view and edit this device's agent config
 * (~/.enoxian/agents.toml) over the loopback API. Edits this machine's own
 * config only; never synced. Switching to `push` (which lets a chat mention run
 * a local process) is gated behind a confirm; adding/removing agents is
 * ordinary launcher config. See docs/concepts/proposals.md.
 */
export default function DeviceSettings({ onClose, circleId, membership, onOpenFile }: Props) {
  const { circles } = useApp()
  const activeCircleId = circleId ?? null
  const scope = circleId ? 'circle' : 'global'
  const [activeTab, setActiveTab] = useState<SettingsTab>('behaviour')
  const auth = useAgentAuthentication(activeTab === 'agents', activeCircleId)
  const [cfg, setCfg] = useState<AgentConfigView | null>(null)
  const [plugins, setPlugins] = useState<AgentPlugin[] | null>(null)
  const [known, setKnown] = useState<DiscoveredAgent[]>([])
  const [connectivity, setConnectivity] = useState<ConnectivitySettings | null>(null)
  const [identity, setIdentityState] = useState<IdentityInfo | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [installingPlugin, setInstallingPlugin] = useState<string | null>(null)

  const [saved, setSaved] = useState(false)
  const [identityDirty, setIdentityDirty] = useState(false)
  const [discard, setDiscard] = useState<(() => void) | null>(null)
  const requestId = useRef(0)
  const mutationBusy = useRef(false)

  // Add-agent form state.
  const [showAdd, setShowAdd] = useState(false)
  const [name, setName] = useState('')
  const [driver, setDriver] = useState<'acp' | 'argv'>('acp')
  const [command, setCommand] = useState('')

  const refresh = useCallback(async () => {
    const request = ++requestId.current
    setError(null)
    try {
      if (activeTab === 'connectivity' && activeCircleId) {
        const next = await getConnectivitySettings(activeCircleId)
        if (request === requestId.current) setConnectivity(next)
      } else if (activeTab === 'device') {
        const next = await getIdentity()
        if (request === requestId.current) setIdentityState(next)
      } else if (activeTab === 'agents' || activeTab === 'behaviour') {
        const next = await getAgentConfigFor(activeCircleId)
        if (request !== requestId.current) return
        setCfg(next)
        if (activeTab === 'agents') {
          const [adapters, discovered] = await Promise.all([getAgentPlugins(), discoverAgents()])
          if (request !== requestId.current) return
          setPlugins(adapters.plugins)
          setKnown(discovered.agents)
        }
      }
    } catch (e) {
      if (request === requestId.current) setError(e instanceof Error ? e.message : 'Unable to load settings')
    }
  }, [activeCircleId, activeTab])

  useEffect(() => {
    void refresh()
    setSaved(false)
    return () => { requestId.current += 1 }
  }, [refresh])

  const run = async (fn: () => Promise<unknown>) => {
    if (mutationBusy.current) return false
    mutationBusy.current = true
    setBusy(true)
    setError(null)
    setSaved(false)
    try {
      await fn()
      await refresh()
      setSaved(true)
      return true
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Unable to save settings')
      return false
    } finally {
      mutationBusy.current = false
      setBusy(false)
    }
  }

  const navigate = (action: () => void) => {
    if (busy) return
    if (identityDirty || (showAdd && (name.trim() || command.trim()))) setDiscard(() => action)
    else action()
  }

  const toggleForceRelay = async () => {
    if (!activeCircleId || !connectivity || busy) return
    setBusy(true)
    setSaved(false)
    setError(null)
    try {
      const next = await setForceRelay(activeCircleId, !connectivity.force_relay)
      setConnectivity({ ...connectivity, ...next })
      setSaved(true)
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
    run(() => addAgent(name.trim(), driver, parts)).then(success => {
      if (!success) return
      setName(''); setCommand(''); setDriver('acp'); setShowAdd(false)
    })
  }

  const applyEngagement = (patch: Patch) =>
    run(() => setEngagement({
      ...(scope === 'circle' && activeCircleId ? { circle_id: activeCircleId } : {}),
      ...patch,
    }))

  const managedNames = new Set((plugins || []).map(plugin => plugin.agent))
  const customAgents = cfg?.agents.filter(agent => !managedNames.has(agent.name)) || []
  const activeCircle = circles.find(circle => circle.circle_id === activeCircleId)

  return (
    <>
    <Dialog onClose={() => navigate(onClose)} className="device-settings-panel"
      title={scope === 'global' ? 'Global settings' : 'Circle settings'} >
        <div className="settings-layout">
          <aside className="settings-sidebar">
            <div className="settings-context">
              <span>{scope === 'global' ? 'GLOBAL SETTINGS' : 'CIRCLE SETTINGS'}</span>
              <strong>{scope === 'global' ? 'This device' : activeCircle?.circle_name ?? 'Circle'}</strong>
            </div>
          <SegmentedTabs
            className="settings-tabs"
            ariaLabel="Settings sections"
            orientation="vertical"
            value={activeTab}
            onChange={tab => navigate(() => { setIdentityDirty(false); setShowAdd(false); setName(''); setCommand(''); setActiveTab(tab) })}
            options={SETTINGS_TABS.filter(tab => scope === 'global' ? tab.value !== 'connectivity' && tab.value !== 'membership' : tab.value === 'behaviour' || tab.value === 'connectivity' || (tab.value === 'membership' && !!membership))}
          />
          <p className="settings-sidebar__note">{scope === 'global' ? 'Defaults for this device across all Circles.' : activeTab === 'membership' ? 'Your participation in this Circle on this device.' : 'This device’s preferences in this Circle.'}</p>
          </aside>
          <div className="ritual-panel__body settings-panel-body flex flex-col gap-4">
          <header className="settings-section-heading">
            <span className="settings-section-heading__scope">{scope === 'global' ? 'THIS DEVICE · GLOBAL' : `THIS DEVICE IN ${activeCircle?.circle_name ?? 'CIRCLE'}`}</span>
            <h2>{{ device: 'Device identity', agents: 'Your agents', behaviour: 'Agent behaviour', membership: 'Circle membership', connectivity: 'Connectivity' }[activeTab]}</h2>
            <p>{{ device: 'How you and this machine appear to others.', agents: 'Connect the agents available on this machine. Used across all Circles.', behaviour: scope === 'global' ? 'Set how agents respond across all Circles.' : 'Tailor how agents respond in this Circle.', membership: 'Pause participation or leave this Circle on this device.', connectivity: 'Manage how this device connects to the current Circle.' }[activeTab]}</p>
          </header>
          {activeTab === 'membership' && membership}
          {error && (
            <div className="file-error" role="alert">
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
                  onDirtyChange={setIdentityDirty}
                  onSave={async patch => { if (await run(() => setIdentity(patch))) setIdentityDirty(false) }}
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
                <p className="settings-scope__note">
                  {scope === 'global'
                    ? 'Defaults for all Circles on this device.'
                    : 'Only affects this device; never shared with the Circle. Unchanged options follow your global defaults.'}
                </p>
                {scope === 'global' && <div className="settings-concurrency">
                  Agents running at once on this device
                  <Select aria-label="Agents running at once on this device" disabled={busy} value={String(cfg.max_concurrent_runs ?? 4)}
                    onChange={value => void run(() => setEngagement({ max_concurrent_runs: Number(value) }))}
                    options={[1, 2, 4, 8, 16, 32].map(n => ({ value: String(n), label: n === 1 ? '1 (serial)' : String(n) }))} />
                  <span className="block text-slate">Shared across all Circles on this device. Extra requests wait for a turn. Restart the daemon to apply a new limit.</span>
                </div>}
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
              {scope === 'global' && <WardenSettings />}
              {scope === 'circle' && activeCircleId && (
                <AgentCards circleId={activeCircleId}
                  onOpenFile={onOpenFile && (path => { onOpenFile(path); onClose() })} />
              )}

            </>
          )}

          {activeTab === 'agents' && auth.error && <p role="alert" className="text-[11px] font-mono">{auth.error}</p>}
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
                              <span className="text-[9px] font-bold px-1.5 py-0.5 bg-obsidian text-alabaster">ENABLED</span>
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
                                className="settings-secondary-action"
                                title={`Disable @${plugin.agent}`}
                                aria-label={`Disable @${plugin.agent}`}
                              >Disable</button>
                            )}
                          </div>
                        </div>

                        {ready && <AgentAuthentication agent={plugin.agent} state={auth.states[plugin.agent]}
                          pending={auth.pending[plugin.agent]} onAction={auth.act} />}
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
                        aria-label="Agent name" placeholder="agent name"
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
                        aria-label="Executable and arguments" placeholder="executable and arguments"
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
                      ? { label: 'INSTALLED', ready: true, detail: about }
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
                        {health.ready && agent.driver === 'acp' && <AgentAuthentication agent={agent.name} state={auth.states[agent.name]}
                          pending={auth.pending[agent.name]} onAction={auth.act} />}
                      </div>
                      <div className="flex items-center gap-2 shrink-0">
                        <span
                          className={`text-[8px] font-bold px-1.5 py-0.5 ${health.ready ? 'bg-obsidian text-alabaster' : 'border border-obsidian/40 text-slate'}`}
                          title={health.ready ? 'command[0] resolves on this machine' : 'This agent cannot start until its command resolves'}
                        >{health.label}</span>
                        <button onClick={() => run(() => removeAgent(agent.name))} disabled={busy} className="settings-secondary-action" aria-label={`Disable @${agent.name}`}>Disable</button>
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

              {/* Members connect directly where the network allows and through
                  one of these relays otherwise; each device homes on the
                  nearest. */}
              <div className="settings-connectivity__availability" aria-label="Relays">
                {connectivity.relays.length === 0 && (
                  <span><i aria-hidden="true" /> NO RELAYS</span>
                )}
                {connectivity.relays.map(url => (
                  <span key={url} className="is-ready" title={url}>
                    <i aria-hidden="true" /> {relayHost(url)}
                  </span>
                ))}
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
      {activeTab !== 'membership' && <div className="settings-save-status" role="status">{busy ? 'Saving changes…' : saved && !error ? 'Changes saved' : {
        device: 'Use Save to update your device identity.',
        agents: 'Agent changes apply across all Circles on this device.',
        behaviour: 'Saved automatically · Press Enter or leave a number field to save.',
        connectivity: 'Routing changes restart this Circle on this device.',
      }[activeTab]}</div>}
    </Dialog>
    {discard && <ConfirmDialog title="Discard unsaved changes?" confirmLabel="Discard changes"
      onClose={() => setDiscard(null)} onConfirm={() => { const action = discard; setDiscard(null); setIdentityDirty(false); action() }}>
      Your unsaved entries will be lost. Saved settings are kept.
    </ConfirmDialog>}
    </>
  )
}
