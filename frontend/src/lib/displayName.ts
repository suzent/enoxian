import type { Member } from '../types'

export function shortenAgentId(agentId: string): string {
  return agentId.replace(/-([A-Za-z0-9]{4})[A-Za-z0-9]{4}$/, '-$1') || agentId
}

/** Human-readable label for a peer. Owner name takes priority; falls back to shortened agent_id. */
export function peerLabel(owner: string, agentId: string): string {
  if (owner?.trim() && owner.length <= 40) return owner.trim()
  return shortenAgentId(agentId) || agentId
}

/** Resolve a display name for an agent_id given the current member list. */
export function resolveDisplayName(agentId: string, members: Member[]): string {
  const member = members.find(m => m.agent_id === agentId)
  if (!member) return shortenAgentId(agentId)
  return peerLabel(member.owner, member.agent_id)
}
