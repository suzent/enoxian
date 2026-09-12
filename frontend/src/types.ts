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

/** A blob referenced by a chat message. Bytes live in the content-addressed
 *  store and are fetched over the sync stream; only this metadata is in CRDT. */
export interface Attachment {
  hash: string
  mime: string
  name: string
  size: number
  /** Intrinsic pixel size, when the server could read it from the header.
   *  Used to reserve layout space so the transcript doesn't jump on load. */
  width?: number
  height?: number
}

/** Delegation provenance. Present from the daemon that posted the message;
 *  absent on system posts and on messages from peers predating the field. */
export interface Relay {
  /** Id of the human message this cascade is rooted in. */
  root: string
  /** Peer that posted that human message. */
  root_peer?: string
  /** Agent turns the cascade has cost so far. */
  spent: number
  /** Agents on this branch, innermost last. A human post has none; a reply
   *  from an agent ends with that agent, so the one before it is who asked. */
  path?: string[]
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
  /** Absent on messages from peers predating attachment support. */
  attachments?: Attachment[]
  /** Delegation provenance; absent on system posts and older peers. */
  relay?: Relay
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
  /** Agents that relayed this work, innermost last, excluding the actor that
   *  wrote the files. `["claude"]` with actor_id "codex" = a person asked
   *  claude, claude asked codex. Empty or absent for direct work. */
  relay_path?: string[]
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
  /** Empty for a native plugin, which pins no version. */
  version: string
  driver: 'acp' | 'argv'
  /** 'npm' for a pinned adapter Enoxian installs, 'native' for a CLI that speaks ACP itself. */
  kind: 'npm' | 'native'
  /** False for a native plugin, which never uses Node. */
  requires_node: boolean
  /** Where to get the CLI when a native plugin is missing. */
  install_url: string
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
  kind: 'typing' | 'seen' | 'working' | 'skipped'
  /** Why, for 'skipped' — e.g. "relay budget spent". */
  detail?: string | null
  message_id: string | null
  updated_at: number
  expires_at: number
}

/** The follow-up window for this device, from `GET /api/chat/engagement`. */
export interface EngagementView {
  /** Agent the next mention-less message routes to, or null if none. */
  agent: string | null
  /** Device that ran the reply, and that a follow-up must wake. */
  peer_id?: string
  /** The agent reply this was resolved from. */
  message_id?: string
  /** Seconds the window lasts; 0 means follow-up routing is off. */
  window_secs: number
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
