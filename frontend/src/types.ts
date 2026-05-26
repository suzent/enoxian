export interface Circle {
  circle_id: string
  circle_name: string
  disabled?: boolean
}

export interface Status {
  circle_id: string
  circle_name: string
  agent_id: string
  workspace: string
  docs: number
}

export interface Member {
  peer_id: string
  owner: string
  agent_id: string
  role: 'admin' | 'member'
}

export interface PendingEntry {
  peer_id: string
  owner: string
  agent_id: string
  requested_at: number
}

export interface Presence {
  agent_id: string
  status: 'online' | 'idle' | 'offline'
  last_seen: string
  current_file: string | null
}

export interface ChatMessage {
  id: string
  agent_id: string
  text: string
  mentions: string[]
  ts: number
}

export interface Task {
  task_id: string
  title: string
  description?: string
  status: 'open' | 'claimed' | 'done'
  created_by: string
  claimed_by?: string
  created_at: string
  updated_at: string
}
