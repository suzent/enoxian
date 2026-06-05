import { useState, useEffect, useRef, useCallback } from 'react'
import type { Presence, Task, Member, PendingEntry } from '../types'
import { getWho, getTasks, createTask, claimTask, doneTask, getFiles, createFile, renameFile, deleteFile, eventStream, inviteCircle, getMembers, getPending, approveMember, rejectMember, removeMember, enableCircle, disableCircle, leaveCircle } from '../api'
import { useApp } from '../context/AppContext'
import { shortenAgentId, peerLabel } from '../lib/displayName'

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


export default function RightPanel({ onFileSelect, selectedFile }: Props) {
  const { activeCircleId, circles, reloadCircles, status } = useApp()
  const [presence, setPresence] = useState<Presence[]>([])
  const [tasks, setTasks] = useState<Task[]>([])
  const [files, setFiles] = useState<string[]>([])
  const [members, setMembers] = useState<Member[]>([])
  const [pending, setPending] = useState<PendingEntry[]>([])
  const [newTaskTitle, setNewTaskTitle] = useState('')
  const [newTaskDesc, setNewTaskDesc] = useState('')
  const [creating, setCreating] = useState(false)
  const [creatingFile, setCreatingFile] = useState(false)
  const [newFilePath, setNewFilePath] = useState('')
  const [fileMenuOpen, setFileMenuOpen] = useState<string | null>(null)
  const [fileActionError, setFileActionError] = useState<string | null>(null)
  const [inviteUri, setInviteUri] = useState<string | null>(null)
  const [inviteConnectivity, setInviteConnectivity] = useState<{peer_addr: string|null, relay_addr: string|null, rendezvous_addr: string|null} | null>(null)
  const [inviteCopied, setInviteCopied] = useState(false)
  const [memberActionError, setMemberActionError] = useState<string | null>(null)
  const [activeTab, setActiveTab] = useState<'members' | 'tasks' | 'files'>('members')
  const selectedFileRef = useRef<string | null>(selectedFile)

  useEffect(() => {
    selectedFileRef.current = selectedFile
  }, [selectedFile])

  const refreshFiles = useCallback(() => {
    if (!activeCircleId) return Promise.resolve()
    return getFiles(activeCircleId)
      .then(data => {
        setFiles(data)
        const selected = selectedFileRef.current
        if (selected && !data.includes(selected)) {
          onFileSelect(null)
        }
      })
      .catch(e => console.error('[files]', e))
  }, [activeCircleId, onFileSelect])

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

    const refreshFilesQueued = () => {
      if (filesInFlight) {
        filesRefreshPending = true
        return
      }
      filesInFlight = true
      getFiles(activeCircleId)
        .then(data => { if (!cancelled) setFiles(data) })
        .catch(e => console.error('[files]', e))
        .finally(() => {
          filesInFlight = false
          if (filesRefreshPending && !cancelled) {
            filesRefreshPending = false
            refreshFilesQueued()
          }
        })
    }

    const scheduleFilesRefresh = () => {
      if (filesTimer !== undefined) window.clearTimeout(filesTimer)
      filesTimer = window.setTimeout(refreshFilesQueued, 150)
    }

    const scheduleTasksRefresh = () => {
      if (tasksTimer !== undefined) window.clearTimeout(tasksTimer)
      tasksTimer = window.setTimeout(refreshTasks, 150)
    }

    const refresh = () => {
      refreshPresence()
      refreshTasks()
      refreshFilesQueued()
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
  }, [activeCircleId, onFileSelect, refreshFiles])

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

  const userGroups = buildUserGroups(members, presence, status?.agent_id ?? '')
  const activeCircle = circles.find(c => c.circle_id === activeCircleId)

  const claim = (taskId: string) => {
    if (!activeCircleId || !status) return
    claimTask(activeCircleId, taskId, status.agent_id).then(refreshTasks).catch(() => {})
  }

  const done = (taskId: string) => {
    if (!activeCircleId || !status) return
    doneTask(activeCircleId, taskId, status.agent_id).then(refreshTasks).catch(() => {})
  }

  const submitFile = async () => {
    const path = newFilePath.trim()
    if (!path || !activeCircleId) return
    setFileActionError(null)
    try {
      await createFile(activeCircleId, path)
      setNewFilePath('')
      setCreatingFile(false)
      await refreshFiles()
      onFileSelect(path)
    } catch (err: any) {
      setFileActionError(err.message)
    }
  }

  const handleRenameFile = async (path: string) => {
    if (!activeCircleId) return
    setFileMenuOpen(null)
    const next = window.prompt('Rename file', path)?.trim()
    if (!next || next === path) return
    setFileActionError(null)
    try {
      await renameFile(activeCircleId, path, next)
      await refreshFiles()
      if (selectedFile === path) onFileSelect(next)
    } catch (err: any) {
      setFileActionError(err.message)
    }
  }

  const handleDeleteFile = async (path: string) => {
    if (!activeCircleId) return
    setFileMenuOpen(null)
    if (!window.confirm(`Delete ${path}?`)) return
    setFileActionError(null)
    try {
      await deleteFile(activeCircleId, path)
      await refreshFiles()
      if (selectedFile === path) onFileSelect(null)
    } catch (err: any) {
      setFileActionError(err.message)
    }
  }

  // Build a simple nested tree from flat paths
  const fileTree = buildTree(files)

  const handleInvite = async () => {
    if (!activeCircleId) return
    if (inviteUri) {
      setInviteUri(null)
      setInviteConnectivity(null)
      setInviteCopied(false)
      return
    }
    try {
      const res = await inviteCircle(activeCircleId)
      setInviteUri(res.invite_uri)
      setInviteConnectivity(res.connectivity ?? null)
    } catch (err: any) {
      alert(`Error generating invite: ${err.message}`)
    }
  }

  const handleToggleCircleEnabled = async () => {
    if (!activeCircle) return
    try {
      if (activeCircle.disabled) await enableCircle(activeCircle.circle_id)
      else await disableCircle(activeCircle.circle_id)
      await reloadCircles()
    } catch (err: any) {
      alert(`Error updating circle: ${err.message}`)
    }
  }

  const handleLeaveCircle = async () => {
    if (!activeCircleId || !activeCircle) return
    if (!window.confirm(`Leave ${activeCircle.circle_name}? Local config will be removed. Workspace files are untouched.`)) return
    try {
      await leaveCircle(activeCircleId)
      await reloadCircles()
    } catch (err: any) {
      alert(`Error leaving circle: ${err.message}`)
    }
  }

  return (
    <aside className="app-right-panel sys-window flex min-h-0 flex-col z-10 overflow-hidden">

      {/* ── Tab bar ─────────────────────────────────────────────────────── */}
      <div className="flex shrink-0 border-b-2 border-obsidian">
        {(['members', 'tasks', 'files'] as const).map((tab, i) => (
          <button
            key={tab}
            onClick={() => setActiveTab(tab)}
            className={`flex-1 py-1.5 text-[9px] font-bold tracking-widest font-mono uppercase ${
              i < 2 ? 'border-r-2 border-obsidian' : ''
            } ${activeTab === tab ? 'bg-obsidian text-alabaster' : 'hover:bg-obsidian/5'}`}
            style={{ transition: 'none' }}
          >
            {tab === 'members' && pending.length > 0 ? `MEMBERS (${pending.length})` : tab.toUpperCase()}
          </button>
        ))}
      </div>

      {/* ── MEMBERS tab ─────────────────────────────────────────────────── */}
      {activeTab === 'members' && (
        <div className="flex flex-col flex-1 min-h-0 overflow-hidden">
          {/* Invite row */}
          <div className="section-header">
            <span>MEMBERS</span>
            <button onClick={handleInvite}>{inviteUri ? 'CLOSE' : 'INVITE'}</button>
          </div>

          {inviteUri && (
            <div className="px-4 py-3 border-b border-dashed border-obsidian/30 text-[11px] font-mono">
              <div className="flex items-center gap-2 mb-2">
                <span className="text-slate text-[9px] font-bold shrink-0">URI</span>
                <span className="flex-1 min-w-0 border border-obsidian/30 px-2 py-1 text-[10px] text-obsidian/70 select-none truncate" title={inviteUri}>
                  {inviteUri.slice(0, 20)}···{inviteUri.slice(-6)}
                </span>
                <button
                  onClick={() => { navigator.clipboard.writeText(inviteUri); setInviteCopied(true); setTimeout(() => setInviteCopied(false), 2000) }}
                  className={`shrink-0 text-[9px] px-2 py-1 border font-bold ${inviteCopied ? 'bg-obsidian text-alabaster border-obsidian' : 'border-obsidian hover:bg-obsidian hover:text-alabaster'}`}
                >{inviteCopied ? 'COPIED ✓' : 'COPY'}</button>
              </div>
              {inviteConnectivity && (() => {
                const wan = inviteConnectivity.peer_addr || inviteConnectivity.relay_addr || inviteConnectivity.rendezvous_addr
                const tags: string[] = []
                if (inviteConnectivity.peer_addr) tags.push('DIRECT')
                if (inviteConnectivity.relay_addr) tags.push('RELAY')
                if (inviteConnectivity.rendezvous_addr) tags.push('RDVZ')
                return (
                  <div className="flex items-center gap-2 text-[9px]">
                    <span className={`font-bold ${wan ? 'text-obsidian' : 'text-slate'}`}>{wan ? '✦ WAN' : '⚠ LAN-ONLY'}</span>
                    {tags.map(t => <span key={t} className="border border-obsidian/40 px-1 text-slate">{t}</span>)}
                  </div>
                )
              })()}
            </div>
          )}

          {/* Pending approvals */}
          {pending.length > 0 && (
            <div className="px-4 py-3 border-b border-dashed border-obsidian/30 flex flex-col gap-2 font-mono text-[11px]">
              <div className="group-label approval-label">AWAITING APPROVAL</div>
              {memberActionError && <div className="file-error">{memberActionError}</div>}
              {pending.map(p => (
                <div key={p.peer_id} className="flex flex-col gap-1 pb-2 border-b border-dashed border-obsidian/20 last:border-0">
                  <div className="flex justify-between items-start gap-1">
                    <div className="flex flex-col min-w-0">
                      <span className="font-bold truncate" title={p.agent_id}>{peerLabel(p.owner, p.agent_id)}</span>
                      <span className="text-[9px] text-slate truncate">{p.device_label || shortenAgentId(p.agent_id)}</span>
                      {p.agents.length > 0 && (
                        <div className="flex flex-wrap gap-1 mt-0.5">
                          {p.agents.map(a => <span key={a} className="text-[9px] text-slate border border-obsidian/20 px-1">{a}</span>)}
                        </div>
                      )}
                    </div>
                    <span className="text-[9px] text-slate shrink-0">{age(p.requested_at.toString())}</span>
                  </div>
                  {isAdmin ? (
                    <div className="flex gap-1 mt-0.5">
                      <button onClick={() => handleApprove(p.peer_id, p.owner)} className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster font-bold">APPROVE</button>
                      <button onClick={() => handleReject(p.peer_id)} className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster font-bold">REJECT</button>
                    </div>
                  ) : <div className="text-[9px] text-slate/60">PENDING APPROVAL</div>}
                </div>
              ))}
            </div>
          )}

          {/* Member list: owner → device → agents */}
          <div className="flex-1 overflow-y-auto overflow-x-hidden px-4 py-3 flex flex-col gap-3 font-mono text-[11px]">
            {userGroups.length === 0 && <div className="text-slate">NO MEMBERS INDEXED</div>}
            {userGroups.map(group => {
              const isGroupSelf = group.devices.some(d => d.isSelf)
              const groupLabel = group.owner && group.owner.length <= 40
                ? group.owner : (group.devices[0]?.displayLabel ?? '—')
              return (
                <div key={group.owner || group.devices[0]?.peer_id} className="flex flex-col gap-1">
                  <div className="flex items-center gap-1 min-w-0">
                    <span className={`font-bold text-[10px] tracking-wide truncate ${isGroupSelf ? 'text-obsidian' : ''}`}>
                      {groupLabel}{isGroupSelf ? ' ✦' : ''}
                    </span>
                  </div>
                  {group.devices.map(device => {
                    const p = device.presence
                    const stale = p ? Date.now() - new Date(p.last_seen).getTime() > 90_000 : false
                    const statusKey = p ? (stale && p.status === 'online' ? 'stale' : p.status) : 'offline'
                    return (
                      <div key={device.peer_id} className="ml-2 flex flex-col gap-0.5 pb-1 border-b border-dashed border-obsidian/15 last:border-0">
                        <div className="flex items-center justify-between gap-2">
                          <div className="flex items-center gap-1.5 min-w-0">
                            <span className={`sigil ${statusKey}`} aria-hidden="true" />
                            <span className="font-bold truncate" title={device.agent_id}>{device.displayLabel}</span>
                            <span className={`text-[9px] font-bold ${device.role === 'admin' ? 'text-obsidian' : 'text-slate/50'}`}>{device.role.toUpperCase()}</span>
                          </div>
                          <div className="flex items-center gap-1 shrink-0">
                            {p?.current_file && <span className="text-[9px] text-slate truncate max-w-[60px]" title={p.current_file}>{p.current_file.split('/').pop()}</span>}
                            {isAdmin && !device.isSelf && <button onClick={() => handleRemove(device.peer_id)} className="text-[9px] text-slate hover:text-obsidian font-bold px-1" title={`Remove ${device.displayLabel}`}>×</button>}
                          </div>
                        </div>
                        {device.agents.length > 0 && (
                          <div className="ml-3 flex flex-wrap gap-1">
                            {device.agents.map(a => <span key={a} className="text-[9px] text-slate border border-obsidian/20 px-1">{a}</span>)}
                          </div>
                        )}
                        {p && <div className="ml-3 text-[9px] text-slate">{age(p.last_seen)}</div>}
                      </div>
                    )
                  })}
                </div>
              )
            })}
          </div>
        </div>
      )}

      {/* ── TASKS tab ───────────────────────────────────────────────────── */}
      {activeTab === 'tasks' && (
        <div className="flex flex-col flex-1 min-h-0 overflow-hidden">
          <div className="section-header">
            <span>TASK QUEUE</span>
            <button onClick={() => setCreating(v => !v)}>{creating ? 'CANCEL' : '+'}</button>
          </div>
          {creating && (
            <div className="px-4 py-3 border-b border-dashed border-obsidian/30 flex flex-col gap-2 font-mono text-[11px]">
              <input autoFocus value={newTaskTitle} onChange={e => setNewTaskTitle(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && submitTask()} placeholder="Task title..."
                className="bg-transparent border border-obsidian px-2 py-1 text-[11px] font-mono focus:outline-none focus:bg-obsidian/5 w-full" />
              <input value={newTaskDesc} onChange={e => setNewTaskDesc(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && submitTask()} placeholder="Description (optional)..."
                className="bg-transparent border border-dashed border-obsidian/50 px-2 py-1 text-[11px] font-mono focus:outline-none w-full" />
              <button onClick={submitTask} className="enox-btn self-start">CREATE</button>
            </div>
          )}
          <div className="flex-1 overflow-y-auto p-4 flex flex-col gap-2 font-mono text-[11px]">
            {tasks.length === 0 && <div className="text-slate">NO ACTIVE TASKS</div>}
            {tasks.map(t => {
              const isMe = t.claimed_by === status?.agent_id
              return (
                <div key={t.task_id} className="flex flex-col gap-1 pb-2 border-b border-dashed border-obsidian/20 last:border-0">
                  <div className="flex justify-between items-start gap-2">
                    <span className={`font-bold leading-tight ${t.status === 'done' ? 'line-through text-slate' : ''}`}>{t.title}</span>
                    <span className={`shrink-0 text-[9px] font-bold px-1 border ${t.status === 'open' ? 'border-obsidian' : t.status === 'claimed' ? 'border-obsidian bg-obsidian text-alabaster' : 'border-slate text-slate'}`}>{t.status.toUpperCase()}</span>
                  </div>
                  {t.description && <div className="text-[9px] text-slate leading-tight">{t.description}</div>}
                  {t.claimed_by && t.status !== 'done' && <div className="text-[9px] text-slate">↳ {t.claimed_by}</div>}
                  <div className="flex gap-2 mt-1">
                    {t.status === 'open' && <button onClick={() => claim(t.task_id)} className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster">CLAIM</button>}
                    {t.status === 'claimed' && isMe && <button onClick={() => done(t.task_id)} className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster">DONE</button>}
                  </div>
                </div>
              )
            })}
          </div>
        </div>
      )}

      {/* ── FILES tab ───────────────────────────────────────────────────── */}
      {activeTab === 'files' && (
        <div className="flex flex-col flex-1 min-h-0 overflow-hidden">
          <div className="section-header">
            <span>FILES</span>
            <button onClick={() => setCreatingFile(v => !v)} title={creatingFile ? 'Cancel' : 'New file'}>{creatingFile ? '×' : '+'}</button>
          </div>
          {creatingFile && (
            <div className="file-action-box">
              <input autoFocus value={newFilePath} onChange={e => setNewFilePath(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && submitFile()} placeholder="docs/notes.md" className="file-input" />
              <button onClick={submitFile} className="file-mini-btn">CREATE</button>
            </div>
          )}
          {fileActionError && <div className="file-error">{fileActionError}</div>}
          <div className="file-list flex-1 overflow-y-auto p-3 font-mono text-[11px]">
            {files.length === 0 && <div className="text-slate">NO ARTIFACTS INDEXED</div>}
            <FileTree nodes={fileTree} onSelect={onFileSelect} onRename={handleRenameFile}
              onDelete={handleDeleteFile} openMenu={fileMenuOpen} onOpenMenu={setFileMenuOpen}
              selected={selectedFile} depth={0} />
          </div>
        </div>
      )}

      {activeCircle && (
        <div className="circle-actions">
          <button
            onClick={handleToggleCircleEnabled}
            className={activeCircle.disabled ? 'circle-actions__primary' : 'circle-actions__secondary'}
            title={activeCircle.disabled ? 'Enable this circle' : 'Disable this circle'}
            data-state={activeCircle.disabled ? 'DISABLED' : 'ENABLED'}
            data-action={activeCircle.disabled ? 'ENABLE' : 'DISABLE'}
          >
            <span className="circle-actions__state">{activeCircle.disabled ? 'DISABLED' : 'ENABLED'}</span>
            <span className="circle-actions__action">{activeCircle.disabled ? 'ENABLE' : 'DISABLE'}</span>
          </button>
          <button
            onClick={handleLeaveCircle}
            className="circle-actions__danger"
            title="Leave this circle"
          >
            LEAVE
          </button>
        </div>
      )}

    </aside>
  )
}

// ── User/device grouping ──────────────────────────────────────────────────────

interface DeviceView {
  peer_id: string
  displayLabel: string
  agent_id: string
  agents: string[]
  role: 'admin' | 'member'
  presence: Presence | null
  isSelf: boolean
}

interface UserGroup {
  owner: string
  devices: DeviceView[]
}

function buildUserGroups(members: Member[], presenceList: Presence[], selfAgentId: string): UserGroup[] {
  const presenceByPeer = new Map<string, Presence>()
  const presenceByAgent = new Map<string, Presence>()
  for (const p of presenceList) {
    if (p.peer_id) presenceByPeer.set(p.peer_id, p)
    presenceByAgent.set(p.agent_id, p)
  }
  const byOwner = new Map<string, DeviceView[]>()
  for (const m of members) {
    const p = presenceByPeer.get(m.peer_id) ?? presenceByAgent.get(m.agent_id) ?? null
    const device: DeviceView = {
      peer_id: m.peer_id,
      displayLabel: m.device_label || shortenAgentId(m.agent_id),
      agent_id: m.agent_id,
      agents: m.agents,
      role: m.role,
      presence: p,
      isSelf: m.agent_id === selfAgentId,
    }
    const list = byOwner.get(m.owner) ?? []
    list.push(device)
    byOwner.set(m.owner, list)
  }
  return Array.from(byOwner.entries()).map(([owner, devices]) => ({ owner, devices }))
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

function FileTree({ nodes, onSelect, onRename, onDelete, openMenu, onOpenMenu, selected, depth }: {
  nodes: TreeNode[]
  onSelect: (path: string) => void
  onRename: (path: string) => void
  onDelete: (path: string) => void
  openMenu: string | null
  onOpenMenu: (path: string | null) => void
  selected: string | null
  depth: number
}) {
  const [open, setOpen] = useState<Set<string>>(new Set())

  return (
    <>
      {nodes.map(n => (
        <div key={n.path}>
          <div
            className={`file-row ${
                          selected === n.path
                            ? 'selected'
                            : ''
                        }`}
            style={{ paddingLeft: `${depth * 12}px` }}
          >
            <button
              className="file-name"
              onClick={() => {
                if (n.isDir) setOpen(s => { const ns = new Set(s); ns.has(n.path) ? ns.delete(n.path) : ns.add(n.path); return ns })
                else onSelect(n.path)
              }}
              title={n.path}
            >
              <span>{n.isDir ? (open.has(n.path) ? '[-] ' : '[+] ') : '    '}{n.name}</span>
            </button>
            {!n.isDir && (
              <span className="file-actions">
                <button
                  className="file-menu-trigger"
                  onClick={() => onOpenMenu(openMenu === n.path ? null : n.path)}
                  title={`More actions for ${n.path}`}
                  aria-label={`More actions for ${n.path}`}
                >
                  ⋮
                </button>
              </span>
            )}
          </div>
          {!n.isDir && openMenu === n.path && (
            <div className="file-menu file-menu-inline">
              <button onClick={() => onRename(n.path)}>Rename</button>
              <button onClick={() => onDelete(n.path)}>Delete</button>
            </div>
          )}
          {n.isDir && open.has(n.path) && (
            <FileTree
              nodes={n.children}
              onSelect={onSelect}
              onRename={onRename}
              onDelete={onDelete}
              openMenu={openMenu}
              onOpenMenu={onOpenMenu}
              selected={selected}
              depth={depth + 1}
            />
          )}
        </div>
      ))}
    </>
  )
}
