import type { Circle, Status, Presence, ChatMessage, Task } from './types'

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
export const claimTask = (id: string, taskId: string, agentId: string) =>
  post(`${api(id)}/claim`, { task_id: taskId, agent_id: agentId })
export const doneTask = (id: string, taskId: string, agentId: string) =>
  post(`${api(id)}/done`, { task_id: taskId, agent_id: agentId })
export const getFiles = (id: string) => get<string[]>(`${api(id)}/files`)

export function chatStream(circleId: string): EventSource {
  return new EventSource(`${api(circleId)}/chat/stream`)
}

export function wsYjsUrl(circleId: string, filePath: string): string {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${location.host}/circles/${circleId}/ws/yjs?path=${encodeURIComponent(filePath)}`
}
