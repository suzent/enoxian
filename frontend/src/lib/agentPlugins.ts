import type { AgentPlugin } from '../types'

/** Whether a plugin belongs in the regular agent list.
 *
 *  Built-in and local plugins always do. A registry agent does only once it
 *  is on this device (installed or configured); until then it is a catalog
 *  entry, browsed separately so forty-odd listings do not bury the agents
 *  the user actually has. */
export function isListedPlugin(p: AgentPlugin): boolean {
  return p.origin !== 'registry' || p.configured || p.legacy_configured || p.state !== 'missing'
}

/** Registry agents this device does not have yet, matching `query`. */
export function availableRegistryPlugins(plugins: AgentPlugin[], query = ''): AgentPlugin[] {
  const q = query.trim().toLowerCase()
  return plugins
    .filter(p => p.origin === 'registry' && !isListedPlugin(p))
    .filter(p => !q || [p.id, p.agent, p.registry?.name ?? '', p.about].some(f => f.toLowerCase().includes(q)))
}

/** What a registry plugin downloads, in words. */
export function pluginSource(p: AgentPlugin): string {
  switch (p.kind) {
    case 'binary': return p.verified ? 'Prebuilt release · checksum verified' : 'Prebuilt release · no published checksum'
    case 'uvx': return `PyPI ${p.package}@${p.version} · via uv`
    case 'npm': return `npm ${p.package}@${p.version}`
    default: return 'Uses your installed CLI'
  }
}
