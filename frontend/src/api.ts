import type { Circle, Status, Presence, ChatMessage, ChatActivity, Task, Member, PendingEntry, Proposal, ProposalDetail, AgentConfigView, DiscoveredAgent, AgentPlugin, ConnectivitySettings } from './types'

const api = (circleId: string) => `/circles/${circleId}/api`

// The daemon injects the local API token into the served HTML as
// window.__ENOX_TOKEN__. Every API call presents it; WebSocket/EventSource
// (which cannot set headers) append it as ?token=.
const TOKEN: string = (window as unknown as { __ENOX_TOKEN__?: string }).__ENOX_TOKEN__ ?? ''

function authHeaders(extra?: Record<string, string>): Record<string, string> {
  return TOKEN ? { Authorization: `Bearer ${TOKEN}`, ...extra } : { ...extra }
}

/** Append the token as a query param for WS/SSE URLs that can't send headers. */
export function withToken(url: string): string {
  if (!TOKEN) return url
  return url + (url.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(TOKEN)
}

async function get<T>(url: string): Promise<T> {
  const res = await fetch(url, { headers: authHeaders() })
  if (!res.ok) {
    let msg = `${res.status} ${url}`
    try {
      const data = await res.json()
      if (data.error) msg = data.error
    } catch {}
    throw new Error(msg)
  }
  return res.json()
}

async function post<T>(url: string, body: unknown): Promise<T> {
  const res = await fetch(url, {
    method: 'POST',
    headers: authHeaders({ 'Content-Type': 'application/json' }),
    body: JSON.stringify(body),
  })
  if (!res.ok) {
    let msg = `${res.status} ${url}`
    try {
      const data = await res.json()
      if (data.error) msg = data.error
    } catch {}
    throw new Error(msg)
  }
  return res.json()
}


export const getCircles = () => get<Circle[]>('/circles')
// Device-level (not circle-scoped): how this device reacts to chat mentions.
export const getAgentConfig = () => get<AgentConfigView>('/api/agent-config')
// Probe well-known agents for local install status (read-only; runs nothing).
export const discoverAgents = () =>
  get<{ agents: DiscoveredAgent[] }>('/api/agent-config/discover')
export const getAgentPlugins = () =>
  get<{ plugins: AgentPlugin[] }>('/api/agent-plugins')
export const installAgentPlugin = (pluginId: string) =>
  post<{ ok: boolean; plugin: string; command: string[] }>(`/api/agent-plugins/${encodeURIComponent(pluginId)}/install`, {})
export const setAgentReaction = (reaction: 'push' | 'pull') =>
  post('/api/agent-config/reaction', { reaction })
export const addAgent = (name: string, driver: string, command: string[], working_dir?: string) =>
  post('/api/agent-config/agents', { name, driver, command, working_dir })
export const removeAgent = (name: string) =>
  post('/api/agent-config/agents/remove', { name })
export const getStatus = (id: string) => get<Status>(`${api(id)}/status`)
export const getConnectivitySettings = (id: string) =>
  get<ConnectivitySettings>(`${api(id)}/connectivity`)
export const setForceRelay = (id: string, forceRelay: boolean) =>
  post<{ force_relay: boolean; active: boolean; restarted: boolean }>(
    `${api(id)}/connectivity`, { force_relay: forceRelay },
  )
export const getWho = (id: string) => get<Presence[]>(`${api(id)}/who`)
export const getChat = (id: string, since?: number) =>
  get<ChatMessage[]>(`${api(id)}/chat${since ? `?since=${since}` : ''}`)
export const postChat = (id: string, text: string, agentId: string) =>
  post(`${api(id)}/chat`, { text, agent_id: agentId })
export const getChatActivity = (id: string) =>
  get<ChatActivity[]>(`${api(id)}/chat/activity`)
export const setChatTyping = (id: string, actorId: string, typing: boolean) =>
  post<{ok: boolean}>(`${api(id)}/chat/activity`, { actor_id: actorId, typing })
export const getTasks = (id: string) => get<Task[]>(`${api(id)}/tasks`)
export const createTask = (id: string, title: string, description: string, agentId: string) =>
  post(`${api(id)}/tasks`, { title, description, created_by: agentId })
export const claimTask = (id: string, taskId: string, agentId: string) =>
  post(`${api(id)}/claim`, { task_id: taskId, agent_id: agentId })
export const doneTask = (id: string, taskId: string, agentId: string) =>
  post(`${api(id)}/done`, { task_id: taskId, agent_id: agentId })
export const getFiles = (id: string) => get<string[]>(`${api(id)}/files`)
export const createFile = (id: string, path: string, content = '') =>
  post<{status: string, path: string}>(`${api(id)}/files/create`, { path, content })
export const renameFile = (id: string, from: string, to: string) =>
  post<{status: string, from: string, to: string}>(`${api(id)}/files/rename`, { from, to })
export const deleteFile = (id: string, path: string) =>
  post<{status: string, path: string}>(`${api(id)}/files/delete`, { path })

// ── Proposals (M14) ──────────────────────────────────────────────────────────
export const getProposals = (id: string) => get<Proposal[]>(`${api(id)}/proposals`)
export const getProposalDetail = (id: string, proposalId: string) =>
  get<ProposalDetail>(`${api(id)}/proposals/${proposalId}`)
export const acceptProposal = (id: string, proposalId: string) =>
  post<{status: string}>(`${api(id)}/proposals/${proposalId}/accept`, {})
export const rejectProposal = (id: string, proposalId: string) =>
  post<{status: string}>(`${api(id)}/proposals/${proposalId}/reject`, {})
export const revertProposal = (id: string, proposalId: string) =>
  post<{status: string}>(`${api(id)}/proposals/${proposalId}/revert`, {})

// ── Member management (M11) ──────────────────────────────────────────────────
export const getMembers = (id: string) => get<Member[]>(`/circles/${id}/members`)
export const getPending = (id: string) => get<PendingEntry[]>(`/circles/${id}/members/pending`)
export const approveMember = (id: string, peerId: string, role: string, owner: string, adminSig: string) =>
  post<{status: string}>(`/circles/${id}/members/approve`, { peer_id: peerId, role, owner, admin_signature: adminSig })
export const rejectMember = (id: string, peerId: string, adminSig: string) =>
  post<{status: string}>(`/circles/${id}/members/reject`, { peer_id: peerId, admin_signature: adminSig })
export const removeMember = (id: string, peerId: string, adminSig: string) =>
  post<{status: string}>(`/circles/${id}/members/remove`, { peer_id: peerId, admin_signature: adminSig })

export const initCircle = (name: string, owner?: string, joinPolicy?: string, dir?: string) =>
  post<{status: string, circle_id?: string}>('/api/init', { name, owner, join_policy: joinPolicy, dir })
export const enterCircle = (target: string, owner?: string, secret?: string, peer?: string, dir?: string) =>
  post<{status: string, circle_id?: string}>('/api/enter', { target, owner, secret, peer, dir })
export const inviteCircle = (id: string) =>
  post<{invite_uri: string, connectivity: {peer_addr: string|null, relay_addr: string|null, rendezvous_addr: string|null}}>(`${api(id)}/invite`, {})
export const enableCircle = (id: string) =>
  post<{status: string}>(`${api(id)}/enable`, {})
export const disableCircle = (id: string) =>
  post<{status: string}>(`${api(id)}/disable`, {})
export const leaveCircle = (id: string) =>
  post<{status: string}>(`${api(id)}/leave`, {})
export function chatStream(circleId: string): EventSource {
  return new EventSource(withToken(`${api(circleId)}/chat/stream`))
}

export function eventStream(circleId: string): EventSource {
  return new EventSource(withToken(`${api(circleId)}/events`))
}

// ── Identity (global, no circle required) ────────────────────────────────────

export interface IdentityInfo {
  device_label: string
  user_handle: string | null
  has_user_key: boolean
}

export const getIdentity = () => get<IdentityInfo>('/api/identity')

export const setIdentity = (body: { device_label?: string; user_handle?: string }) =>
  post<{ status: string }>('/api/identity', body)

export const linkDevice = (handle: string, mnemonic: string) =>
  post<{ status: string; user_handle: string }>('/api/identity/link', { handle, mnemonic })

export const createUserIdentity = (user_handle: string) =>
  post<{ status: string; handle: string; mnemonic: string; note: string }>(
    '/api/identity/create-user', { user_handle }
  )

// ─────────────────────────────────────────────────────────────────────────────

export function wsYjsUrl(circleId: string, filePath: string): string {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  return withToken(`${proto}://${location.host}/circles/${circleId}/ws/yjs?path=${encodeURIComponent(filePath)}`)
}
