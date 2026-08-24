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
  removed: boolean
}

export interface ConnectivitySettings {
  force_relay: boolean
  active: boolean
  relay_configured: boolean
  rendezvous_configured: boolean
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
  connections: PeerConnection[]
}

export interface PeerConnection {
  kind: 'lan' | 'tailscale' | 'public' | 'relay'
  address: string
}

export interface ChatMessage {
  id: string
  agent_id: string
  text: string
  mentions: string[]
  ts: number
  /** Peer that posted the message. Disambiguates agent replies, whose agent_id
   *  is a bare name several devices may share. Empty from older peers. */
  peer_id?: string
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
  status: 'ready' | 'missing' | 'runtime_download'
}

export interface AgentPlugin {
  id: string
  agent: string
  version: string
  driver: 'acp' | 'argv'
  package: string
  about: string
  source: string
  state: 'missing' | 'installing' | 'broken' | 'ready'
  configured: boolean
  legacy_configured: boolean
  executable: string
  node_runtime_installed: boolean
  node_runtime_version: string | null
  runtime_program: string | null
  runtime_installed: boolean | null
  runtime_login_command: string | null
}

export interface ChatActivity {
  activity_id: string
  actor_id: string
  peer_id: string
  kind: 'typing' | 'seen' | 'working'
  message_id: string | null
  updated_at: number
  expires_at: number
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
