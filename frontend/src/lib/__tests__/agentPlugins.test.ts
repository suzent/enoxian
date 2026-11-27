import { describe, it, expect } from 'vitest'
import type { AgentPlugin } from '../../types'
import { availableRegistryPlugins, isListedPlugin, pluginSource } from '../agentPlugins'

const plugin = (extra: Partial<AgentPlugin>): AgentPlugin => ({
  id: 'x', agent: 'x', version: '1.0.0', driver: 'acp', kind: 'npm', requires_node: true, install_url: '',
  package: 'x', about: 'An agent', source: '', state: 'missing', configured: false, legacy_configured: false,
  executable: '', node_runtime_installed: true, node_runtime_version: null,
  runtime_program: null, runtime_installed: null, runtime_login_command: null, ...extra,
})

describe('agent plugin listing', () => {
  it('lists built-ins always and registry agents only once they are on this device', () => {
    expect(isListedPlugin(plugin({}))).toBe(true)
    expect(isListedPlugin(plugin({ origin: 'builtin' }))).toBe(true)
    expect(isListedPlugin(plugin({ origin: 'registry' }))).toBe(false)
    expect(isListedPlugin(plugin({ origin: 'registry', state: 'ready' }))).toBe(true)
    expect(isListedPlugin(plugin({ origin: 'registry', configured: true }))).toBe(true)
    expect(isListedPlugin(plugin({ origin: 'registry', state: 'broken' }))).toBe(true)
  })

  it('searches available registry agents by id, handle, name and description', () => {
    const all = [
      plugin({ id: 'gemini', agent: 'gemini', origin: 'registry', registry: { name: 'Gemini CLI', license: '' } }),
      plugin({ id: 'goose', agent: 'goose', origin: 'registry', about: 'Open source agent by Block' }),
      plugin({ id: 'codex-acp', agent: 'codex', origin: 'builtin' }),
    ]
    expect(availableRegistryPlugins(all).map(p => p.id)).toEqual(['gemini', 'goose'])
    expect(availableRegistryPlugins(all, 'gemini cli').map(p => p.id)).toEqual(['gemini'])
    expect(availableRegistryPlugins(all, 'BLOCK').map(p => p.id)).toEqual(['goose'])
    expect(availableRegistryPlugins(all, 'codex')).toEqual([])
  })

  it('describes what each kind downloads', () => {
    expect(pluginSource(plugin({ kind: 'binary', verified: true }))).toMatch(/checksum verified/)
    expect(pluginSource(plugin({ kind: 'binary', verified: false }))).toMatch(/no published checksum/)
    expect(pluginSource(plugin({ kind: 'uvx', package: 'fast-agent-acp' }))).toBe('PyPI fast-agent-acp@1.0.0 · via uv')
    expect(pluginSource(plugin({ package: '@google/gemini-cli' }))).toBe('npm @google/gemini-cli@1.0.0')
  })
})
