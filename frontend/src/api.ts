import type { Circle, Status, Presence, ChatMessage, Task, Member, PendingEntry } from './types'

const api = (circleId: string) => `/circles/${circleId}/api`

async function get<T>(url: string): Promise<T> {
  const res = await fetch(url)
  if (!res.ok) throw new Error(`${res.status} ${url}`)
  return res.json()
}

async function post<T>(url: string, body: unknown): Promise<T> {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`${res.status} ${url}`)
  return res.json()
}


export const getCircles = () => get<Circle[]>('/circles')
export const getStatus = (id: string) => get<Status>(`${api(id)}/status`)
export const getWho = (id: string) => get<Presence[]>(`${api(id)}/who`)
export const getChat = (id: string, since?: number) =>
  get<ChatMessage[]>(`${api(id)}/chat${since ? `?since=${since}` : ''}`)
export const postChat = (id: string, text: string, agentId: string) =>
  post(`${api(id)}/chat`, { text, agent_id: agentId })
export const getTasks = (id: string) => get<Task[]>(`${api(id)}/tasks`)
export const createTask = (id: string, title: string, description: string, agentId: string) =>
  post(`${api(id)}/tasks`, { title, description, created_by: agentId })
export const claimTask = (id: string, taskId: string, agentId: string) =>
  post(`${api(id)}/claim`, { task_id: taskId, agent_id: agentId })
export const doneTask = (id: string, taskId: string, agentId: string) =>
  post(`${api(id)}/done`, { task_id: taskId, agent_id: agentId })
export const getFiles = (id: string) => get<string[]>(`${api(id)}/files`)

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
  post<{status: string}>('/api/enter', { target, owner, secret, peer, dir })
export const inviteCircle = (id: string) =>
  post<{invite_uri: string, connectivity: {peer_addr: string|null, relay_addr: string|null, rendezvous_addr: string|null}}>(`${api(id)}/invite`, {})
export const enableCircle = (id: string) =>
  post<{status: string}>(`${api(id)}/enable`, {})
export const disableCircle = (id: string) =>
  post<{status: string}>(`${api(id)}/disable`, {})
export const leaveCircle = (id: string) =>
  post<{status: string}>(`${api(id)}/leave`, {})
export function chatStream(circleId: string): EventSource {
  return new EventSource(`${api(circleId)}/chat/stream`)
}

export function eventStream(circleId: string): EventSource {
  return new EventSource(`${api(circleId)}/events`)
}

export function wsYjsUrl(circleId: string, filePath: string): string {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${location.host}/circles/${circleId}/ws/yjs?path=${encodeURIComponent(filePath)}`
}
