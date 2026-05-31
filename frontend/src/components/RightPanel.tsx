import { useState, useEffect, useRef, useCallback } from 'react'
import type { Presence, Task, Member, PendingEntry } from '../types'
import { getWho, getTasks, createTask, claimTask, doneTask, getFiles, eventStream, inviteCircle, getMembers, getPending, approveMember, rejectMember, removeMember } from '../api'
import { useApp } from '../context/AppContext'
import { agentColor } from '../lib/agentColor'

interface Props {
  onFileSelect: (path: string | null) => void
  selectedFile: string | null
}

function age(isoStr: string) {
  const secs = Math.floor((Date.now() - new Date(isoStr).getTime()) / 1000)
  if (secs < 60) return 'just now'
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`
  return `${Math.floor(secs / 3600)}h ago`
}

/** Shorten the auto-generated peer suffix from 8 → 4 chars.
 *  "human-Kj4RQm48" → "human-Kj4R"
 *  Custom names that don't match the pattern are returned as-is. */
function shortenAgentId(agentId: string): string {
  return agentId.replace(/-([A-Za-z0-9]{4})[A-Za-z0-9]{4}$/, '-$1') || agentId
}

/** Best human-readable label for a peer.
 *  Priority: explicit owner name → shortened agent id */
function peerLabel(owner: string, agentId: string): string {
  if (owner?.trim()) return owner.trim()
  return shortenAgentId(agentId) || agentId
}

export default function RightPanel({ onFileSelect, selectedFile }: Props) {
  const { activeCircleId, status } = useApp()
  const [presence, setPresence] = useState<Presence[]>([])
  const [tasks, setTasks] = useState<Task[]>([])
  const [files, setFiles] = useState<string[]>([])
  const [members, setMembers] = useState<Member[]>([])
  const [pending, setPending] = useState<PendingEntry[]>([])
  const [newTaskTitle, setNewTaskTitle] = useState('')
  const [newTaskDesc, setNewTaskDesc] = useState('')
  const [creating, setCreating] = useState(false)
  const [inviteUri, setInviteUri] = useState<string | null>(null)
  const [inviteConnectivity, setInviteConnectivity] = useState<{peer_addr: string|null, relay_addr: string|null, rendezvous_addr: string|null} | null>(null)
  const [inviteCopied, setInviteCopied] = useState(false)
  const [memberActionError, setMemberActionError] = useState<string | null>(null)
  const selectedFileRef = useRef<string | null>(selectedFile)

  useEffect(() => {
    selectedFileRef.current = selectedFile
  }, [selectedFile])

  useEffect(() => {
    setPresence([])
    setTasks([])
    setFiles([])
    setMembers([])
    setPending([])
    if (!activeCircleId) return

    let cancelled = false

    let filesTimer: number | undefined
    let tasksTimer: number | undefined
    let filesInFlight = false
    let tasksInFlight = false
    let filesRefreshPending = false
    let tasksRefreshPending = false

    const refreshPresence = () => {
      getWho(activeCircleId).then(data => { if (!cancelled) setPresence(data) }).catch(() => {})
    }

    const refreshMembers = () => {
      getMembers(activeCircleId).then(data => { if (!cancelled) setMembers(data) }).catch(() => {})
      getPending(activeCircleId).then(data => { if (!cancelled) setPending(data) }).catch(() => {})
    }

    const refreshTasks = () => {
      if (tasksInFlight) {
        tasksRefreshPending = true
        return
      }
      tasksInFlight = true
      getTasks(activeCircleId)
        .then(data => { if (!cancelled) setTasks(data) })
        .catch(() => {})
        .finally(() => {
          tasksInFlight = false
          if (tasksRefreshPending && !cancelled) {
            tasksRefreshPending = false
            refreshTasks()
          }
        })
    }

    const refreshFiles = () => {
      if (filesInFlight) {
        filesRefreshPending = true
        return
      }
      filesInFlight = true
      getFiles(activeCircleId)
        .then(data => {
          if (cancelled) return
          setFiles(data)
          const selected = selectedFileRef.current
          if (selected && !data.includes(selected)) {
            onFileSelect(null)
          }
        })
        .catch(e => console.error('[files]', e))
        .finally(() => {
          filesInFlight = false
          if (filesRefreshPending && !cancelled) {
            filesRefreshPending = false
            refreshFiles()
          }
        })
    }

    const scheduleFilesRefresh = () => {
      if (filesTimer !== undefined) window.clearTimeout(filesTimer)
      filesTimer = window.setTimeout(refreshFiles, 150)
    }

    const scheduleTasksRefresh = () => {
      if (tasksTimer !== undefined) window.clearTimeout(tasksTimer)
      tasksTimer = window.setTimeout(refreshTasks, 150)
    }

    const refresh = () => {
      refreshPresence()
      refreshTasks()
      refreshFiles()
      refreshMembers()
    }

    refresh()
    const id = setInterval(refresh, 15_000)
    const es = eventStream(activeCircleId)
    es.addEventListener('message', e => {
      try {
        const data = JSON.parse(e.data)
        if (data.type === 'file_deleted' && typeof data.path === 'string') {
          setFiles(prev => prev.filter(path => path !== data.path && !path.startsWith(`${data.path}/`)))
          const selected = selectedFileRef.current
          if (selected === data.path || selected?.startsWith(`${data.path}/`)) {
            onFileSelect(null)
          }
        }
        if (data.type === 'file_updated' || data.type === 'file_deleted') {
          scheduleFilesRefresh()
        }
        if (data.type === 'task_created' || data.type === 'task_claimed' || data.type === 'task_done') {
          scheduleTasksRefresh()
        }
        if (data.type === 'member_joined' || data.type === 'member_removed' || data.type === 'member_pending') {
          refreshMembers()
        }
      } catch {}
    })
    return () => {
      cancelled = true
      clearInterval(id)
      if (filesTimer !== undefined) window.clearTimeout(filesTimer)
      if (tasksTimer !== undefined) window.clearTimeout(tasksTimer)
      es.close()
    }
  }, [activeCircleId, onFileSelect])

  const refreshTasks = useCallback(() => {
    if (activeCircleId) getTasks(activeCircleId).then(setTasks).catch(() => {})
  }, [activeCircleId])

  const submitTask = () => {
    const title = newTaskTitle.trim()
    if (!title || !activeCircleId || !status) return
    createTask(activeCircleId, title, newTaskDesc.trim(), status.agent_id)
      .then(() => { setNewTaskTitle(''); setNewTaskDesc(''); setCreating(false); refreshTasks() })
      .catch(() => {})
  }

  // Determine if the current user is admin
  const isAdmin = members.some(m => m.agent_id === status?.agent_id && m.role === 'admin')
    || members.some(m => m.peer_id && m.role === 'admin' && m.agent_id === status?.agent_id)

  const refreshMembers = useCallback(() => {
    if (!activeCircleId) return
    getMembers(activeCircleId).then(setMembers).catch(() => {})
    getPending(activeCircleId).then(setPending).catch(() => {})
  }, [activeCircleId])

  const handleApprove = async (peerId: string, owner: string) => {
    if (!activeCircleId) return
    setMemberActionError(null)
    try {
      // The frontend can't sign with admin.key (server-side only).
      // We call the approve endpoint; the daemon validates the admin key itself
      // when the request carries no sig — only works in "api mode" where daemon
      // auto-signs if it holds admin.key.
      await approveMember(activeCircleId, peerId, 'member', owner, '')
      refreshMembers()
    } catch (err: any) {
      setMemberActionError(`approve failed: ${err.message}`)
    }
  }

  const handleReject = async (peerId: string) => {
    if (!activeCircleId) return
    setMemberActionError(null)
    try {
      await rejectMember(activeCircleId, peerId, '')
      refreshMembers()
    } catch (err: any) {
      setMemberActionError(`reject failed: ${err.message}`)
    }
  }

  const handleRemove = async (peerId: string) => {
    if (!activeCircleId) return
    setMemberActionError(null)
    try {
      await removeMember(activeCircleId, peerId, '')
      refreshMembers()
    } catch (err: any) {
      setMemberActionError(`remove failed: ${err.message}`)
    }
  }

  const local = presence.filter(p => p.agent_id === status?.agent_id)
  const remote = presence.filter(p => p.agent_id !== status?.agent_id)

  const claim = (taskId: string) => {
    if (!activeCircleId || !status) return
    claimTask(activeCircleId, taskId, status.agent_id).then(refreshTasks).catch(() => {})
  }

  const done = (taskId: string) => {
    if (!activeCircleId || !status) return
    doneTask(activeCircleId, taskId, status.agent_id).then(refreshTasks).catch(() => {})
  }

  // Build a simple nested tree from flat paths
  const fileTree = buildTree(files)

  const handleInvite = async () => {
    if (!activeCircleId) return
    try {
      const res = await inviteCircle(activeCircleId)
      setInviteUri(res.invite_uri)
      setInviteConnectivity(res.connectivity ?? null)
    } catch (err: any) {
      alert(`Error generating invite: ${err.message}`)
    }
  }

  return (
    <aside className="app-right-panel flex min-h-0 flex-col border-l-2 border-obsidian bg-alabaster/85 z-10 overflow-hidden">

      {/* ── Presence ─────────────────────────────────────────────────────── */}
      <div className="section-header flex justify-between items-center pr-3">
        <span>Structural Entities</span>
        <button onClick={handleInvite} className="text-[9px] font-bold font-mono hover:underline text-alabaster/70 hover:text-alabaster">[+] INVITE</button>
      </div>
      {inviteUri && (
        <div className="px-4 py-3 border-b border-dashed border-obsidian/30 text-[11px] font-mono">
          {/* Truncated URI + copy button */}
          <div className="flex items-center gap-2 mb-2">
            <span className="text-slate text-[9px] font-bold shrink-0">INVITE</span>
            <span
              className="flex-1 min-w-0 bg-obsidian/8 border border-obsidian/30 px-2 py-1 text-[10px] truncate text-obsidian/70 select-none"
              title={inviteUri}
            >
              {inviteUri.slice(0, 24)}···{inviteUri.slice(-6)}
            </span>
            <button
              onClick={() => {
                navigator.clipboard.writeText(inviteUri)
                setInviteCopied(true)
                setTimeout(() => setInviteCopied(false), 2000)
              }}
              className={`shrink-0 text-[9px] px-2 py-1 border font-bold transition-colors ${
                inviteCopied
                  ? 'bg-obsidian text-alabaster border-obsidian'
                  : 'border-obsidian hover:bg-obsidian hover:text-alabaster'
              }`}
            >
              {inviteCopied ? 'COPIED ✓' : 'COPY'}
            </button>
          </div>

          {/* Connectivity summary */}
          {inviteConnectivity && (() => {
            const wan = inviteConnectivity.peer_addr || inviteConnectivity.relay_addr || inviteConnectivity.rendezvous_addr
            const tags: string[] = []
            if (inviteConnectivity.peer_addr) tags.push('DIRECT')
            if (inviteConnectivity.relay_addr) tags.push('RELAY')
            if (inviteConnectivity.rendezvous_addr) tags.push('RDVZ')
            return (
              <div className="flex items-center gap-2 text-[9px]">
                <span className={`font-bold ${wan ? 'text-obsidian' : 'text-slate'}`}>
                  {wan ? '✦ WAN' : '⚠ LAN-ONLY'}
                </span>
                {tags.map(t => (
                  <span key={t} className="border border-obsidian/40 px-1 text-slate">{t}</span>
                ))}
              </div>
            )
          })()}

          <div className="mt-2 text-right">
            <button
              onClick={() => { setInviteUri(null); setInviteConnectivity(null); setInviteCopied(false) }}
              className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster font-bold"
            >
              CLOSE
            </button>
          </div>
        </div>
      )}
      <div className="p-4 border-b border-dashed border-obsidian/30 flex flex-col gap-4 font-mono text-[11px]">
        {local.length > 0 && (
          <div className="flex flex-col gap-2">
            <div className="group-label">LOCAL HOST</div>
            {local.map(p => <PresenceRow key={p.agent_id} p={p} />)}
          </div>
        )}
        {remote.length > 0 && (
          <div className="flex flex-col gap-2">
            <div className="group-label">REMOTE PEERS</div>
            {remote.map(p => <PresenceRow key={p.agent_id} p={p} />)}
          </div>
        )}
        {presence.length === 0 && <div className="text-slate">NO ENTITIES DETECTED</div>}
      </div>

      {/* ── Members ──────────────────────────────────────────────────────── */}
      <div className="section-header border-t-2 border-obsidian">
        Circle Members
        {pending.length > 0 && (
          <span className="ml-2 bg-red-600 text-alabaster text-[9px] font-bold px-1.5 py-0.5">
            {pending.length} PENDING
          </span>
        )}
      </div>

      {/* Pending approval queue — only visible when there are requests */}
      {pending.length > 0 && (
        <div className="px-4 py-3 border-b border-dashed border-obsidian/30 flex flex-col gap-2 font-mono text-[11px]">
          <div className="group-label text-red-600">AWAITING APPROVAL</div>
          {memberActionError && (
            <div className="text-red-600 text-[9px] font-bold bg-red-50 border border-red-400 px-2 py-1">
              {memberActionError}
            </div>
          )}
          {pending.map(p => (
            <div key={p.peer_id} className="flex flex-col gap-1 pb-2 border-b border-dashed border-obsidian/20 last:border-0">
              <div className="flex justify-between items-start gap-1">
                <div className="flex flex-col min-w-0">
                  <span className="font-bold truncate" title={p.agent_id}>
                    {peerLabel(p.owner, p.agent_id)}
                  </span>
                  <span className="text-[9px] text-slate font-mono truncate" title={p.peer_id}>
                    {shortenAgentId(p.agent_id)}
                  </span>
                </div>
                <span className="text-[9px] text-slate shrink-0">{age(p.requested_at.toString())}</span>
              </div>
              {isAdmin ? (
                <div className="flex gap-1 mt-0.5">
                  <button
                    onClick={() => handleApprove(p.peer_id, p.owner)}
                    className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster font-bold"
                  >
                    APPROVE
                  </button>
                  <button
                    onClick={() => handleReject(p.peer_id)}
                    className="text-[9px] border border-red-600 text-red-600 px-2 py-0.5 hover:bg-red-600 hover:text-alabaster font-bold"
                  >
                    REJECT
                  </button>
                </div>
              ) : (
                <div className="text-[9px] text-slate/60">PENDING APPROVAL</div>
              )}
            </div>
          ))}
        </div>
      )}

      {/* Member list */}
      <div className="px-4 py-3 border-b border-dashed border-obsidian/30 flex flex-col gap-2 font-mono text-[11px] max-h-[180px] overflow-y-auto">
        {members.length === 0 && <div className="text-slate">NO MEMBERS INDEXED</div>}
        {members.map(m => {
          const isSelf = m.agent_id === status?.agent_id
          const name = peerLabel(m.owner, m.agent_id)
          // Always show device name as subtitle; omit only if it would duplicate the primary label
          const deviceLabel = shortenAgentId(m.agent_id)
          const subtitle = deviceLabel !== name ? deviceLabel : null
          return (
            <div key={m.peer_id} className="flex justify-between items-center gap-2 pb-1 border-b border-dashed border-obsidian/20 last:border-0">
              <div className="flex flex-col min-w-0">
                <span className={`font-bold truncate ${isSelf ? 'text-obsidian' : ''}`} title={m.agent_id}>
                  {name}{isSelf ? ' ✦' : ''}
                </span>
                <span className={`text-[9px] font-mono ${m.role === 'admin' ? 'text-obsidian font-bold' : 'text-slate'}`}>
                  {m.role.toUpperCase()}{subtitle ? ` · ${subtitle}` : ''}
                </span>
              </div>
              {isAdmin && !isSelf && (
                <button
                  onClick={() => handleRemove(m.peer_id)}
                  className="shrink-0 text-[9px] text-slate hover:text-red-600 font-bold px-1"
                  title={`Remove ${name}`}
                >
                  ×
                </button>
              )}
            </div>
          )
        })}
      </div>

      {/* ── Tasks ────────────────────────────────────────────────────────── */}
      <div className="section-header border-t-2 border-obsidian flex justify-between items-center pr-3">
        <span>Task Queue</span>
        <button
          onClick={() => setCreating(v => !v)}
          className="text-[9px] font-bold font-mono hover:underline"
        >{creating ? '✕ CANCEL' : '+ NEW'}</button>
      </div>

      {creating && (
        <div className="px-4 py-3 border-b border-dashed border-obsidian/30 flex flex-col gap-2 font-mono text-[11px]">
          <input
            autoFocus
            value={newTaskTitle}
            onChange={e => setNewTaskTitle(e.target.value)}
            onKeyDown={e => e.key === 'Enter' && submitTask()}
            placeholder="Task title..."
            className="bg-transparent border border-obsidian px-2 py-1 text-[11px] font-mono focus:outline-none focus:bg-obsidian/5 w-full"
          />
          <input
            value={newTaskDesc}
            onChange={e => setNewTaskDesc(e.target.value)}
            onKeyDown={e => e.key === 'Enter' && submitTask()}
            placeholder="Description (optional)..."
            className="bg-transparent border border-dashed border-obsidian/50 px-2 py-1 text-[11px] font-mono focus:outline-none w-full"
          />
          <button onClick={submitTask} className="enox-btn self-start">CREATE</button>
        </div>
      )}

      <div className="p-4 border-b border-dashed border-obsidian/30 flex flex-col gap-2 font-mono text-[11px] max-h-[220px] overflow-y-auto">
        {tasks.length === 0 && <div className="text-slate">NO ACTIVE TASKS</div>}
        {tasks.map(t => {
          const isMe = t.claimed_by === status?.agent_id
          return (
            <div key={t.task_id} className="flex flex-col gap-1 pb-2 border-b border-dashed border-obsidian/20 last:border-0">
              <div className="flex justify-between items-start gap-2">
                <span className={`font-bold leading-tight ${t.status === 'done' ? 'line-through text-slate' : ''}`}>
                  {t.title}
                </span>
                <span className={`shrink-0 text-[9px] font-bold px-1 border ${
                  t.status === 'open'    ? 'border-obsidian' :
                  t.status === 'claimed' ? 'border-obsidian bg-obsidian text-alabaster' :
                  'border-slate text-slate'
                }`}>{t.status.toUpperCase()}</span>
              </div>
              {t.description && (
                <div className="text-[9px] text-slate leading-tight">{t.description}</div>
              )}
              {t.claimed_by && t.status !== 'done' && (
                <div className="text-[9px] text-slate">↳ {t.claimed_by}</div>
              )}
              <div className="flex gap-2 mt-1">
                {t.status === 'open' && (
                  <button
                    onClick={() => claim(t.task_id)}
                    className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster"
                  >CLAIM</button>
                )}
                {t.status === 'claimed' && isMe && (
                  <button
                    onClick={() => done(t.task_id)}
                    className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster"
                  >DONE</button>
                )}
              </div>
            </div>
          )
        })}
      </div>

      {/* ── File tree ────────────────────────────────────────────────────── */}
      <div className="section-header border-t-2 border-obsidian">Artifact Filesystem</div>
      <div className="flex-1 overflow-y-auto p-4 font-mono text-[11px]">
        {files.length === 0 && <div className="text-slate">NO ARTIFACTS INDEXED</div>}
        <FileTree nodes={fileTree} onSelect={onFileSelect} selected={selectedFile} depth={0} />
      </div>
    </aside>
  )
}

function PresenceRow({ p }: { p: Presence }) {
  const stale = Date.now() - new Date(p.last_seen).getTime() > 90_000
  const status = stale && p.status === 'online' ? 'stale' : p.status
  const dot = status === 'online' ? '●' : status === 'idle' || status === 'stale' ? '◑' : '○'
  const color = agentColor(p.agent_id)
  return (
    <div className="flex flex-col gap-0.5">
      <div className="flex justify-between items-baseline">
        <span className="font-bold" title={p.agent_id}>@{shortenAgentId(p.agent_id)}</span>
        <div className="flex gap-2 items-baseline">
          <span className="text-[9px] font-bold" style={{ color }}>{dot} {status.toUpperCase()}</span>
          <span className="text-[9px] text-slate">{age(p.last_seen)}</span>
        </div>
      </div>
      {p.current_file && (
        <div className="text-[9px] text-slate truncate" title={p.current_file}>
          AT {p.current_file}
        </div>
      )}
    </div>
  )
}

// ── File tree ─────────────────────────────────────────────────────────────────

interface TreeNode {
  name: string
  path: string
  children: TreeNode[]
  isDir: boolean
}

function buildTree(paths: string[]): TreeNode[] {
  const root: TreeNode = { name: '', path: '', children: [], isDir: true }
  for (const p of paths) {
    const parts = p.split('/')
    let node = root
    for (let i = 0; i < parts.length; i++) {
      const isLast = i === parts.length - 1
      const part = parts[i]
      let child = node.children.find(c => c.name === part)
      if (!child) {
        child = { name: part, path: parts.slice(0, i + 1).join('/'), children: [], isDir: !isLast }
        node.children.push(child)
      }
      node = child
    }
  }
  return root.children
}

function FileTree({ nodes, onSelect, selected, depth }: {
  nodes: TreeNode[]
  onSelect: (path: string) => void
  selected: string | null
  depth: number
}) {
  const [open, setOpen] = useState<Set<string>>(new Set())

  return (
    <>
      {nodes.map(n => (
        <div key={n.path}>
          <div
            className={`flex justify-between py-1 border-b border-dashed border-obsidian/20 cursor-pointer
                        transition-colors ${
                          selected === n.path
                            ? 'bg-obsidian text-alabaster'
                            : 'hover:bg-slate/15 hover:text-obsidian'
                        }`}
            style={{ paddingLeft: `${depth * 12}px` }}
            onClick={() => {
              if (n.isDir) setOpen(s => { const ns = new Set(s); ns.has(n.path) ? ns.delete(n.path) : ns.add(n.path); return ns })
              else onSelect(n.path)
            }}
          >
            <span>{n.isDir ? (open.has(n.path) ? '▾ ' : '▸ ') : '  '}{n.name}</span>
          </div>
          {n.isDir && open.has(n.path) && (
            <FileTree nodes={n.children} onSelect={onSelect} selected={selected} depth={depth + 1} />
          )}
        </div>
      ))}
    </>
  )
}
