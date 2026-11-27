import { useState } from 'react'
import type { AgentPlugin } from '../types'
import { ConfirmDialog } from './ui/Dialog'
import { availableRegistryPlugins, pluginSource } from '../lib/agentPlugins'

interface Props {
  plugins: AgentPlugin[]
  busy: boolean
  installing: string | null
  /** Handles already configured on this device, including custom agents. */
  takenHandles?: string[]
  onInstall: (plugin: AgentPlugin, options: { handle?: string; allow_unverified?: boolean }) => void
  onRefresh: () => void
}

const HANDLE = /^[A-Za-z0-9._-]+$/

/** What stops this device from running a plugin, if anything. */
function missingRuntime(p: AgentPlugin): string | null {
  if (p.requires_node && !p.node_runtime_installed) return 'Needs Node.js 22+ with npm'
  if (p.requires_uv && !p.uv_installed) return 'Needs uv'
  return null
}

/**
 * Agents from the public ACP Registry that this device does not have yet.
 *
 * Adding one is an explicit install of third-party code, so it goes through a
 * confirmation that says what will be downloaded, from where, and under which
 * licence — and lets the user pick the @handle it answers to.
 */
export default function AgentRegistry({ plugins, busy, installing, takenHandles = [], onInstall, onRefresh }: Props) {
  const [query, setQuery] = useState('')
  const [pending, setPending] = useState<AgentPlugin | null>(null)
  const [handle, setHandle] = useState('')
  const all = availableRegistryPlugins(plugins)
  const shown = availableRegistryPlugins(plugins, query)
  const taken = new Set([
    ...plugins.filter(p => p.origin !== 'registry' || p.configured).map(p => p.agent),
    ...takenHandles,
  ])

  const open = (p: AgentPlugin) => { setPending(p); setHandle(p.agent) }
  const handleError = !pending ? null
    : !HANDLE.test(handle.trim().replace(/^@/, '')) ? 'Use letters, digits, ".", "_" or "-".'
      : taken.has(handle.trim().replace(/^@/, '')) ? `@${handle.trim().replace(/^@/, '')} is already in use on this device.`
        : null

  return (
    <details className="agent-registry font-mono border-t border-obsidian/30 pt-2">
      <summary className="cursor-pointer text-[10px] font-bold select-none">
        ACP REGISTRY <span className="text-slate font-normal">· {all.length} more agents</span>
      </summary>
      <div className="mt-3 flex flex-col gap-2">
        <div className="flex items-center gap-2">
          <input value={query} onChange={e => setQuery(e.target.value)}
            aria-label="Search the ACP Registry" placeholder="search agents"
            className="flex-1 border border-obsidian px-2 py-1 text-[11px] focus:outline-none focus:bg-obsidian/5" />
          <button type="button" className="agent-btn" disabled={busy} onClick={onRefresh}
            title="Fetch the current registry index">REFRESH</button>
        </div>
        <p className="text-[9px] text-slate">
          Published by third parties through the{' '}
          <a href="https://github.com/agentclientprotocol/registry" target="_blank" rel="noreferrer" className="underline">ACP Registry</a>.
          Each is pinned to the version you add; nothing downloads when it is mentioned.
        </p>
        {all.length === 0 && (
          <div className="text-[9px] text-slate">The registry has not been fetched yet. Refresh to try again.</div>
        )}
        {all.length > 0 && shown.length === 0 && (
          <div className="text-[9px] text-slate">No registry agent matches “{query}”.</div>
        )}
        <div className="divide-y divide-obsidian/20">
          {shown.map(p => {
            const runtime = missingRuntime(p)
            const isInstalling = installing === p.id
            return (
              <div key={p.id} className="agent-card agent-registry__item">
                <div className="agent-card__name">
                  {p.registry?.icon && <img src={p.registry.icon} alt="" width={14} height={14} className="agent-registry__icon" />}
                  {p.registry?.name ?? p.id}
                  <span>@{p.agent} · v{p.version}</span>
                </div>
                <div className="agent-card__status" title={p.about}>
                  {p.about}
                  <span className="agent-registry__meta">
                    {pluginSource(p)}{p.registry?.license ? ` · ${p.registry.license}` : ''}
                  </span>
                </div>
                <div className="agent-card__actions">
                  <button type="button" className="agent-btn" disabled={busy || !!installing || !!runtime}
                    title={runtime ?? `Add @${p.agent}`} aria-label={`Add ${p.registry?.name ?? p.id}`}
                    onClick={() => open(p)}>{isInstalling ? 'ADDING…' : 'ADD'}</button>
                </div>
                {runtime && <p className="agent-card__note">{runtime}{p.requires_uv
                  ? <> — install it from <a href="https://docs.astral.sh/uv/" target="_blank" rel="noreferrer">docs.astral.sh/uv</a>.</>
                  : <> — install it from <a href="https://nodejs.org/en/download" target="_blank" rel="noreferrer">nodejs.org</a>.</>}</p>}
                {isInstalling && (
                  <div className="agent-card__progress h-1 overflow-hidden bg-obsidian/10" role="progressbar" aria-label={`Installing ${p.registry?.name ?? p.id}`}>
                    <div className="h-full w-2/3 bg-obsidian animate-pulse" />
                  </div>
                )}
              </div>
            )
          })}
        </div>
      </div>

      {pending && (
        <ConfirmDialog title={`Add ${pending.registry?.name ?? pending.id}?`}
          confirmLabel={pending.verified === false ? 'Add unverified' : 'Add agent'}
          onClose={() => setPending(null)}
          onConfirm={() => {
            if (handleError) return
            const chosen = handle.trim().replace(/^@/, '')
            onInstall(pending, {
              ...(chosen !== pending.agent ? { handle: chosen } : {}),
              ...(pending.verified === false ? { allow_unverified: true } : {}),
            })
            setPending(null)
          }}>
          <p>This downloads third-party code from the ACP Registry and runs it when the agent is mentioned.</p>
          <dl className="agent-registry__facts">
            <dt>Version</dt><dd>v{pending.version}</dd>
            <dt>Source</dt><dd>{pluginSource(pending)}</dd>
            {pending.registry?.license && <><dt>License</dt><dd>{pending.registry.license}</dd></>}
            {(pending.registry?.repository || pending.registry?.website) && <>
              <dt>Project</dt>
              <dd><a href={pending.registry.repository || pending.registry.website} target="_blank" rel="noreferrer">
                {pending.registry.repository || pending.registry.website}</a></dd>
            </>}
          </dl>
          {pending.verified === false && (
            <p role="alert"><strong>Its publisher gives no checksum for this release</strong>, so Enoxian cannot verify the download.</p>
          )}
          <label className="agent-registry__handle">
            Mention it as
            <input value={handle} onChange={e => setHandle(e.target.value)} aria-label="Handle"
              aria-invalid={!!handleError} />
          </label>
          {handleError && <p role="alert" className="agent-registry__error">{handleError}</p>}
        </ConfirmDialog>
      )}
    </details>
  )
}
