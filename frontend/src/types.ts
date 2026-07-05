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
  device_label: string
  agents: string[]
  role: 'admin' | 'member'
}

export interface PendingEntry {
  peer_id: string
  owner: string
  agent_id: string
  device_label: string
  agents: string[]
  requested_at: number
}

export interface Presence {
  agent_id: string
  status: 'online' | 'idle' | 'offline'
  last_seen: string
  current_file: string | null
  peer_id: string
}

export interface ChatMessage {
  id: string
  agent_id: string
  text: string
  mentions: string[]
  ts: number
}

export interface Proposal {
  id: string
  circle_id: string
  base_snapshot: string
  result_snapshot: string
  changed_paths: string[]
  status: 'pending' | 'accepted' | 'synced' | 'conflicted' | 'rejected' | 'reverted'
  source: string
  actor_id: string | null
  actor_hint: string | null
  confidence: string
  origin_peer_id: string
  origin_device: string
  created_at: string
}

export interface ProposalFileDiff {
  path: string
  change: 'added' | 'removed' | 'modified'
  before: string | null
  after: string | null
  binary: boolean
}

export interface ProposalDetail extends Proposal {
  files: ProposalFileDiff[]
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

// Read-only view of this device's ~/.enoxian/agents.toml — how it reacts to
// chat @mentions. Editing stays file-only (the `push` reaction is the toggle
// that lets a mention run a local process).
export interface AgentSummary {
  name: string
  driver: string
  command: string[]
  working_dir: string | null
  // Whether command[0] resolves on this machine's PATH right now.
  installed: boolean
}

export interface AgentConfigView {
  reaction: 'push' | 'pull'
  config_path: string
  configured: boolean
  agents: AgentSummary[]
}

// A well-known agent candidate the backend probed for on this machine.
export interface DiscoveredAgent {
  name: string
  driver: 'acp' | 'argv'
  command: string[]
  about: string
  installed: boolean
  configured: boolean
}
