import { X, UserPlus, FilePlus, ListPlus, ChevronDown } from 'lucide-react'
import InviteLink from './InviteLink'
import { useState, useEffect, useRef, useCallback } from 'react'
import type { Presence, Task, Member, PendingEntry, Proposal } from '../types'
import { getWho, getTasks, createTask, claimTask, doneTask, getFiles, createFile, renameFile, deleteFile, eventStream, inviteCircle, getMembers, getPending, approveMember, rejectMember, removeMember, enableCircle, disableCircle, leaveCircle, getProposals } from '../api'
import ProposalsTab from './ProposalsTab'
import { FileQuickView } from './EditorPanel'
import { useApp } from '../context/AppContext'
import { shortenAgentId, peerLabel } from '../lib/displayName'
import SegmentedTabs, { type SegmentedTabOption } from './ui/SegmentedTabs'

interface Props {
  activityRef?: (element: HTMLDivElement | null) => void
  onFileSelect: (path: string | null) => void
  selectedFile: string | null
  activeTab: RightPanelTab
  onClose: () => void
}

export type RightPanelTab = 'members' | 'activity' | 'tasks' | 'workspace'
type WorkspaceView = 'files' | 'history'

function age(isoStr: string) {
  const secs = Math.floor((Date.now() - new Date(isoStr).getTime()) / 1000)
  if (secs < 60) return 'just now'
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`
  return `${Math.floor(secs / 3600)}h ago`
}

const CONNECTION_BADGE: Record<NonNullable<Presence['connections']>[number]['kind'], string> = {
  lan: 'border-emerald-700 text-emerald-700',
  tailscale: 'border-sky-700 text-sky-700',
  public: 'border-amber-700 text-amber-700',
  relay: 'border-obsidian bg-obsidian text-alabaster',
}


export default function RightPanel({ activityRef, onFileSelect, selectedFile, activeTab, onClose }: Props) {
  const { activeCircleId, circles, reloadCircles, status } = useApp()
  const [presence, setPresence] = useState<Presence[]>([])
  const [tasks, setTasks] = useState<Task[]>([])
  const [files, setFiles] = useState<string[]>([])
  const [members, setMembers] = useState<Member[]>([])
  const [pending, setPending] = useState<PendingEntry[]>([])
  const [proposals, setProposals] = useState<Proposal[]>([])
  const [newTaskTitle, setNewTaskTitle] = useState('')
  const [newTaskDesc, setNewTaskDesc] = useState('')
  const [creating, setCreating] = useState(false)
  const [taskActionError, setTaskActionError] = useState<string | null>(null)
  const [showCompletedTasks, setShowCompletedTasks] = useState(false)
  const [workspaceView, setWorkspaceView] = useState<WorkspaceView>('files')
  const [creatingFile, setCreatingFile] = useState(false)
  const [newFilePath, setNewFilePath] = useState('')
  const [fileMenuOpen, setFileMenuOpen] = useState<string | null>(null)
  const [fileActionError, setFileActionError] = useState<string | null>(null)
  const [previewFile, setPreviewFile] = useState<string | null>(null)
  const [openFolders, setOpenFolders] = useState<Set<string>>(() => new Set())
  const [inviteUri, setInviteUri] = useState<string | null>(null)
  const [inviteConnectivity, setInviteConnectivity] = useState<{peer_addr: string|null, relay_addr: string|null, rendezvous_addr: string|null} | null>(null)
  const [longInviteUri, setLongInviteUri] = useState<string | undefined>()
  const [inviteNote, setInviteNote] = useState<string | null>(null)
  const [inviteLoading, setInviteLoading] = useState(false)
  const inviteRequestRef = useRef(0)
  const [memberActionError, setMemberActionError] = useState<string | null>(null)
  const [confirmModal, setConfirmModal] = useState<{ title: string; subject: string; body?: string; onConfirm: () => void } | null>(null)
  const [renameModal, setRenameModal] = useState<{ path: string; value: string } | null>(null)
  const selectedFileRef = useRef<string | null>(selectedFile)
  const activeCircleIdRef = useRef<string | null>(activeCircleId)
  activeCircleIdRef.current = activeCircleId

  useEffect(() => {
    selectedFileRef.current = selectedFile
    if (selectedFile) setPreviewFile(null)
  }, [selectedFile])

  useEffect(() => {
    setInviteUri(null)
    setLongInviteUri(undefined)
    setInviteNote(null)
    setInviteConnectivity(null)
    setInviteLoading(false)
    return () => { inviteRequestRef.current += 1 }
  }, [activeCircleId])

  const openFileInCenter = useCallback((path: string | null) => {
    setPreviewFile(null)
    onFileSelect(path)
  }, [onFileSelect])

  const previewFileInSidebar = useCallback((path: string) => {
    if (selectedFileRef.current) {
      openFileInCenter(path)
      return
    }
    onFileSelect(null)
    setPreviewFile(path)
  }, [onFileSelect, openFileInCenter])

  const refreshFiles = useCallback(() => {
    if (!activeCircleId) return Promise.resolve()
    const requestedCircleId = activeCircleId
    return getFiles(requestedCircleId)
      .then(data => {
        if (activeCircleIdRef.current !== requestedCircleId) return
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
    setProposals([])
    setPreviewFile(null)
    setOpenFolders(new Set())
    setShowCompletedTasks(false)
    setWorkspaceView('files')
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

    const refreshProposals = () => {
      getProposals(activeCircleId).then(data => { if (!cancelled) setProposals(data) }).catch(() => {})
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
      refreshProposals()
    }

    refresh()
    const id = setInterval(refresh, 15_000)
    const es = eventStream(activeCircleId)
    es.addEventListener('message', e => {
      if (cancelled) return
      try {
        const data = JSON.parse(e.data)
        if (data.type === 'file_deleted' && typeof data.path === 'string') {
          setFiles(prev => prev.filter(path => path !== data.path && !path.startsWith(`${data.path}/`)))
          setPreviewFile(current => current === data.path || current?.startsWith(`${data.path}/`) ? null : current)
          const selected = selectedFileRef.current
          if (selected === data.path || selected?.startsWith(`${data.path}/`)) {
            onFileSelect(null)
          }
        }
        if (data.type === 'file_updated' || data.type === 'file_deleted') {
          scheduleFilesRefresh()
        }
        if (
          data.type === 'task_created' ||
          data.type === 'task_claimed' ||
          data.type === 'task_unclaimed' ||
          data.type === 'task_done'
        ) {
          scheduleTasksRefresh()
        }
        if (data.type === 'member_added' || data.type === 'member_removed' || data.type === 'member_pending') {
          refreshMembers()
        }
        if (data.type === 'proposal_created' || data.type === 'proposal_updated') {
          refreshProposals()
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

  useEffect(() => {
    if (previewFile && files.length > 0 && !files.includes(previewFile)) setPreviewFile(null)
  }, [files, previewFile])

  const refreshProposalsNow = useCallback(() => {
    if (!activeCircleId) return
    const requestedCircleId = activeCircleId
    getProposals(requestedCircleId)
      .then(data => {
        if (activeCircleIdRef.current === requestedCircleId) setProposals(data)
      })
      .catch(() => {})
  }, [activeCircleId])

  const submitTask = () => {
    const title = newTaskTitle.trim()
    if (!title || !activeCircleId || !status) return
    setTaskActionError(null)
    createTask(activeCircleId, title, newTaskDesc.trim(), status.agent_id)
      .then(() => { setNewTaskTitle(''); setNewTaskDesc(''); setCreating(false); refreshTasks() })
      .catch((err: any) => setTaskActionError(err.message || 'Unable to create task'))
  }

  // Determine if the current user is admin
  const isAdmin = members.some(m => m.agent_id === status?.agent_id && m.role === 'admin')
    || members.some(m => m.peer_id && m.role === 'admin' && m.agent_id === status?.agent_id)

  // Used after a member action the user just took. Unlike the background
  // poll, a failure here must be visible: silently swallowing it leaves the
  // roster showing the pre-action state, so a request that actually succeeded
  // looks like it did nothing.
  const refreshMembers = useCallback(() => {
    if (!activeCircleId) return
    Promise.all([
      getMembers(activeCircleId).then(setMembers),
      getPending(activeCircleId).then(setPending),
    ]).catch((err: any) => {
      setMemberActionError(`could not refresh members: ${err.message}`)
    })
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
  const activeTasks = tasks
    .filter(task => task.status !== 'done')
    .sort((a, b) => {
      if (a.status !== b.status) return a.status === 'claimed' ? -1 : 1
      return new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime()
    })
  const completedTasks = tasks
    .filter(task => task.status === 'done')
    .sort((a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime())
  const actionableChanges = proposals.filter(proposal =>
    proposal.status === 'pending' || proposal.status === 'conflicted',
  )

  const claim = (taskId: string) => {
    if (!activeCircleId || !status) return
    setTaskActionError(null)
    claimTask(activeCircleId, taskId, status.agent_id).then(refreshTasks).catch(err => setTaskActionError(err.message || 'Unable to claim task'))
  }

  const done = (taskId: string) => {
    if (!activeCircleId || !status) return
    setTaskActionError(null)
    doneTask(activeCircleId, taskId, status.agent_id).then(refreshTasks).catch(err => setTaskActionError(err.message || 'Unable to complete task'))
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

  const handleRenameFile = (path: string) => {
    if (!activeCircleId) return
    setFileMenuOpen(null)
    setRenameModal({ path, value: path })
  }

  const commitRename = async () => {
    if (!activeCircleId || !renameModal) return
    const next = renameModal.value.trim()
    if (!next || next === renameModal.path) { setRenameModal(null); return }
    setFileActionError(null)
    try {
      await renameFile(activeCircleId, renameModal.path, next)
      await refreshFiles()
      if (selectedFile === renameModal.path) onFileSelect(next)
      if (previewFile === renameModal.path) setPreviewFile(next)
      setRenameModal(null)
    } catch (err: any) {
      setFileActionError(err.message)
      setRenameModal(null)
    }
  }

  const handleDeleteFile = (path: string) => {
    if (!activeCircleId) return
    setFileMenuOpen(null)
    setConfirmModal({
      title: 'DELETE FILE',
      subject: path,
      onConfirm: async () => {
        setConfirmModal(null)
        setFileActionError(null)
        try {
          await deleteFile(activeCircleId, path)
          await refreshFiles()
          if (selectedFile === path) onFileSelect(null)
          if (previewFile === path) setPreviewFile(null)
        } catch (err: any) {
          setFileActionError(err.message)
        }
      },
    })
  }

  // Build a simple nested tree from flat paths
  const fileTree = buildTree(files)

  const handleInvite = async () => {
    if (!activeCircleId || inviteLoading) return
    if (inviteUri) {
      setInviteUri(null)
      setInviteConnectivity(null)
      return
    }
    const request = ++inviteRequestRef.current
    setInviteLoading(true)
    setMemberActionError(null)
    try {
      const res = await inviteCircle(activeCircleId)
      if (request !== inviteRequestRef.current) return
      setInviteUri(res.invite_uri)
      setLongInviteUri(res.long_invite_uri)
      setInviteNote(res.short_note ?? null)
      setInviteConnectivity(res.connectivity ?? null)
    } catch (err: any) {
      if (request === inviteRequestRef.current) setMemberActionError(err.message || 'Unable to create invite')
    } finally {
      if (request === inviteRequestRef.current) setInviteLoading(false)
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

  const handleLeaveCircle = () => {
    if (!activeCircleId || !activeCircle) return
    setConfirmModal({
      title: 'LEAVE CIRCLE',
      subject: activeCircle.circle_name,
      body: 'Local config will be removed. Workspace files are untouched.',
      onConfirm: async () => {
        setConfirmModal(null)
        try {
          await leaveCircle(activeCircleId)
          await reloadCircles()
        } catch (err: any) {
          setConfirmModal({ title: 'ERROR', subject: err.message, onConfirm: () => setConfirmModal(null) })
        }
      },
    })
  }

  const workspaceTabs: SegmentedTabOption<WorkspaceView>[] = [
    { value: 'files', content: 'FILES' },
    {
      value: 'history',
      content: <><span>HISTORY</span>{actionableChanges.length > 0 && <span className="is-attention">{actionableChanges.length}</span>}</>,
    },
  ]

  return (
    <>
    <aside className="app-right-panel sys-window flex min-h-0 flex-col z-10 overflow-hidden">

      <div className="section-header right-panel-title">
        <span>{activeTab}</span>
        <button type="button" className="context-close" onClick={onClose} aria-label="Close circle details"><X size={16} /></button>
      </div>
      <div className="right-panel-body">
        <div className="right-panel-content">

      {/* ── MEMBERS tab ─────────────────────────────────────────────────── */}
      {activeTab === 'members' && (
        <div className="sidebar-members flex flex-col min-h-0 overflow-hidden">
          <div className="context-intro">
            <p>People, devices and agents in this Circle.</p>
            <button className="context-action" onClick={handleInvite} disabled={inviteLoading} aria-busy={inviteLoading} aria-expanded={!!inviteUri}>
              <UserPlus size={15} />{inviteLoading ? 'Creating invite…' : inviteUri ? 'Hide invite link' : 'Invite to Circle'}
            </button>
          </div>
          {inviteUri && (
            <div className="member-invitation">
              <InviteLink key={inviteUri} uri={inviteUri} longUri={longInviteUri} note={inviteNote} />
              {inviteConnectivity && (() => {
                const wan = inviteConnectivity.peer_addr || inviteConnectivity.relay_addr || inviteConnectivity.rendezvous_addr
                const tags: string[] = []
                if (inviteConnectivity.peer_addr) tags.push('DIRECT')
                if (inviteConnectivity.relay_addr) tags.push('RELAY')
                if (inviteConnectivity.rendezvous_addr) tags.push('RDVZ')
                return (
                  <div className="invite-connection-status">
                    <span className={wan ? '' : 'is-muted'}>{wan ? '● Reachable' : '○ Local network only'}</span>
                    {tags.map(t => <span key={t} className="panel-action-form__tag">{t}</span>)}
                  </div>
                )
              })()}
            </div>
          )}
          {memberActionError && <div className="panel-feedback panel-feedback--error" role="alert"><strong>ERROR</strong><span>{memberActionError}</span></div>}

          {/* Pending approvals */}
          {pending.length > 0 && (
            <div className="px-4 py-3 border-b border-dashed border-obsidian/30 flex flex-col gap-2 font-mono text-[11px]">
              <div className="group-label approval-label">{pending.every(p => p.automatic) ? 'AUTOMATIC ADMISSION' : 'AWAITING APPROVAL'}</div>
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
                  {p.approval_error && <div role="alert" className="text-[10px] break-words">{p.approval_error}</div>}
                  {p.automatic && <div className="text-[9px] text-slate">{p.approval_error ? 'Automatic admission will retry.' : 'Completing automatic admission…'}</div>}
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
                    const isOnline = statusKey !== 'offline'
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
                            {device.agents.map(a => <span key={a} className="text-[9px] text-slate border border-obsidian/20 px-1">{a}{device.ambientAgents.includes(a) ? ' · listening' : ''}</span>)}
                          </div>
                        )}
                        {p && <div className="ml-3 text-[9px] text-slate">{age(p.last_seen)}</div>}
                        {isOnline && (
                          <div className="ml-3 flex flex-wrap gap-1" aria-label="Active connection routes">
                            {device.isSelf ? (
                              <span className="border border-slate/50 px-1 text-[8px] font-bold text-slate" title="This device">LOCAL</span>
                            ) : p?.connections.length ? (
                              p.connections.map(connection => (
                                <span
                                  key={connection.kind}
                                  className={`border px-1 text-[8px] font-bold ${CONNECTION_BADGE[connection.kind]}`}
                                  title={connection.address}
                                >
                                  {connection.kind.toUpperCase()}
                                </span>
                              ))
                            ) : (
                              <span className="border border-slate/40 px-1 text-[8px] font-bold text-slate/70" title="No direct connection is currently observed by this device">UNKNOWN</span>
                            )}
                          </div>
                        )}
                      </div>
                    )
                  })}
                </div>
              )
            })}
          </div>
          {activeCircle && <details className="member-circle-settings">
            <summary>Circle settings <ChevronDown size={14} /></summary>
            <div>
              <strong>{activeCircle.disabled ? 'Circle disabled' : 'Circle enabled'}</strong>
              <p>{activeCircle.disabled ? 'Enable to resume participation on this device.' : 'Disable to pause participation on this device.'}</p>
              <button type="button" onClick={handleToggleCircleEnabled}>{activeCircle.disabled ? 'Enable circle' : 'Disable circle'}</button>
              <p>Leaving removes the local configuration. Workspace files stay on this device.</p>
              <button type="button" onClick={handleLeaveCircle}>Leave circle…</button>
            </div>
          </details>}
        </div>
      )}

      <div ref={activityRef} className="sidebar-agent-activity" hidden={activeTab !== 'activity'} />

      {/* ── TASKS tab ───────────────────────────────────────────────────── */}
      {activeTab === 'tasks' && (
        <div className="flex flex-col flex-1 min-h-0 overflow-hidden">
          <div className="context-intro">
            <p>Coordinate work and track who is handling it.</p>
            <button className="context-action" onClick={() => setCreating(v => !v)} aria-expanded={creating}><ListPlus size={15} />{creating ? 'Cancel new task' : 'New task'}</button>
          </div>
          {creating && (
            <form className="panel-action-form" onSubmit={e => { e.preventDefault(); submitTask() }}>
              <div className="panel-action-form__heading">
                <strong>NEW TASK</strong>
                <span>Add work to this circle</span>
              </div>
              <label className="panel-action-form__field">
                <span>TITLE</span>
                <input autoFocus value={newTaskTitle} onChange={e => setNewTaskTitle(e.target.value)}
                  placeholder="Describe the outcome" className="panel-action-form__input" />
              </label>
              <label className="panel-action-form__field">
                <span>DESCRIPTION <small>OPTIONAL</small></span>
                <input value={newTaskDesc} onChange={e => setNewTaskDesc(e.target.value)}
                  placeholder="Add useful context" className="panel-action-form__input" />
              </label>
              <div className="panel-action-form__actions">
                <button type="submit" disabled={!newTaskTitle.trim()} className="panel-action-form__button panel-action-form__button--primary">CREATE TASK</button>
                <button type="button" onClick={() => setCreating(false)} className="panel-action-form__button">CANCEL</button>
              </div>
            </form>
          )}
          {taskActionError && <div className="panel-feedback panel-feedback--error" role="alert"><strong>ERROR</strong><span>{taskActionError}</span></div>}
          <div className="task-summary" aria-label="Task summary">
            <span><strong>{activeTasks.length}</strong> ACTIVE</span>
            <span><strong>{activeTasks.filter(task => task.status === 'claimed').length}</strong> CLAIMED</span>
            <span><strong>{completedTasks.length}</strong> COMPLETED</span>
          </div>
          <div className="task-list flex-1 overflow-y-auto font-mono text-[11px]">
            {activeTasks.length === 0 && !creating && <PanelEmpty title="No active tasks" detail="Use New task to describe an outcome. Members can claim it and mark it done." />}
            {activeTasks.map(t => {
              const isMe = t.claimed_by === status?.agent_id
              return (
                <div key={t.task_id} className="task-card">
                  <div className="flex justify-between items-start gap-2">
                    <span className="font-bold leading-tight">{t.title}</span>
                    <span className={`shrink-0 text-[9px] font-bold px-1 border ${t.status === 'open' ? 'border-obsidian' : t.status === 'claimed' ? 'border-obsidian bg-obsidian text-alabaster' : 'border-slate text-slate'}`}>{t.status.toUpperCase()}</span>
                  </div>
                  {t.description && <div className="task-card__description">{t.description}</div>}
                  {t.claimed_by && t.status !== 'done' && <div className="text-[9px] text-slate">↳ {t.claimed_by}</div>}
                  <div className="flex gap-2 mt-1">
                    {t.status === 'open' && <button onClick={() => claim(t.task_id)} className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster">CLAIM</button>}
                    {t.status === 'claimed' && isMe && <button onClick={() => done(t.task_id)} className="text-[9px] border border-obsidian px-2 py-0.5 hover:bg-obsidian hover:text-alabaster">DONE</button>}
                  </div>
                </div>
              )
            })}
            {completedTasks.length > 0 && (
              <section className="task-completed">
                <button
                  type="button"
                  className="task-completed__toggle"
                  onClick={() => setShowCompletedTasks(value => !value)}
                  aria-expanded={showCompletedTasks}
                >
                  <span>COMPLETED</span>
                  <span>{completedTasks.length}</span>
                  <span aria-hidden="true">{showCompletedTasks ? '⌃' : '⌄'}</span>
                </button>
                {showCompletedTasks && (
                  <div className="task-completed__list">
                    {completedTasks.map(task => (
                      <div key={task.task_id} className="task-card task-card--completed">
                        <span className="task-card__check" aria-hidden="true">✓</span>
                        <span>{task.title}</span>
                        <time dateTime={task.updated_at}>{age(task.updated_at)}</time>
                      </div>
                    ))}
                  </div>
                )}
              </section>
            )}
          </div>
        </div>
      )}

      {/* ── WORKSPACE tab ───────────────────────────────────────────────── */}
      {activeTab === 'workspace' && (
        previewFile ? (
          <FileQuickView
            filePath={previewFile}
            onOpen={() => openFileInCenter(previewFile)}
            onClose={() => setPreviewFile(null)}
            full
          />
        ) : (
        <div className="workspace-browser flex flex-col flex-1 min-h-0 overflow-hidden">
          <SegmentedTabs
            className="workspace-switch"
            ariaLabel="Workspace views"
            value={workspaceView}
            onChange={setWorkspaceView}
            options={workspaceTabs}
          />
          {workspaceView === 'files' && (
        <div className="flex flex-col flex-1 min-h-0 overflow-hidden">
          <div className="context-intro context-intro--files">
            <p>Shared files in this Circle.</p>
            <button className="context-action" onClick={() => {
              if (creatingFile) setNewFilePath('')
              setCreatingFile(v => !v)
              setFileActionError(null)
            }} aria-expanded={creatingFile}><FilePlus size={15} />{creatingFile ? 'Cancel new file' : 'New file'}</button>
          </div>
          {fileActionError && !creatingFile && <div className="panel-feedback panel-feedback--error" role="alert">{fileActionError}</div>}
          <div className="workspace-files-layout">
          <div className="file-list flex-1 overflow-y-auto px-3 py-2 font-mono text-[11px]">
            {files.length === 0 && !creatingFile && <PanelEmpty title="No shared files yet" detail="Create a shared file with New file above." />}
            {(files.length > 0 || creatingFile) && (
              <div className="border border-obsidian/30">
                {creatingFile && (
                  <>
                    <form className="file-row file-row--creating" onSubmit={e => { e.preventDefault(); submitFile() }}>
                      <FileIcon name={newFilePath || 'untitled'} isDir={false} isOpen={false} />
                      <input
                        autoFocus
                        value={newFilePath}
                        onChange={e => { setNewFilePath(e.target.value); setFileActionError(null) }}
                        onKeyDown={e => {
                          if (e.key === 'Escape') {
                            e.preventDefault()
                            setNewFilePath('')
                            setCreatingFile(false)
                            setFileActionError(null)
                          }
                        }}
                        placeholder="filename or path"
                        className="file-create-input"
                        aria-label="New file path"
                        aria-invalid={!!fileActionError}
                        aria-describedby={fileActionError ? 'file-create-error' : undefined}
                        spellCheck={false}
                      />
                      <button type="submit" className="file-create-commit" disabled={!newFilePath.trim()}>Create</button>
                    </form>
                    {fileActionError && <div id="file-create-error" className="file-create-error" role="alert">{fileActionError}</div>}
                  </>
                )}
                <FileTree nodes={fileTree} onSelect={previewFileInSidebar} onOpen={openFileInCenter} onRename={handleRenameFile}
                  onDelete={handleDeleteFile} openMenu={fileMenuOpen} onOpenMenu={setFileMenuOpen}
                  selected={previewFile} opened={selectedFile} openFolders={openFolders}
                  onToggleFolder={path => setOpenFolders(current => {
                    const next = new Set(current)
                    next.has(path) ? next.delete(path) : next.add(path)
                    return next
                  })}
                  depth={0} />
              </div>
            )}
          </div>
          </div>
        </div>
          )}
          {workspaceView === 'history' && activeCircleId && (
            <ProposalsTab
              circleId={activeCircleId}
              proposals={proposals}
              onChanged={refreshProposalsNow}
            />
          )}
        </div>
        )
      )}



        </div>
      </div>
    </aside>

    {/* ── Confirm modal (leave / delete) ──────────────────────────────── */}
    {confirmModal && (
      <div className="ritual-modal-backdrop">
        <div className="ritual-panel sys-window">
          <button onClick={() => setConfirmModal(null)} className="ritual-panel__close" aria-label="Close">×</button>
          <div className="ritual-panel__header">{confirmModal.title}</div>
          <div className="ritual-panel__body">
            <div className="ritual-divider" />
            <code className="block font-mono text-[11px] font-bold border border-obsidian px-2 py-1 mb-3 bg-white truncate">{confirmModal.subject}</code>
            {confirmModal.body && <p className="font-mono text-[10px] text-slate mb-3 leading-relaxed">{confirmModal.body}</p>}
            <div className="ritual-actions">
              <button className="ritual-btn ritual-btn--primary" onClick={confirmModal.onConfirm}>CONFIRM</button>
              <button className="ritual-btn ritual-btn--secondary" onClick={() => setConfirmModal(null)}>CANCEL</button>
            </div>
          </div>
        </div>
      </div>
    )}

    {/* ── Rename modal ────────────────────────────────────────────────── */}
    {renameModal && (
      <div className="ritual-modal-backdrop">
        <div className="ritual-panel sys-window">
          <button onClick={() => setRenameModal(null)} className="ritual-panel__close" aria-label="Close">×</button>
          <form onSubmit={e => { e.preventDefault(); commitRename() }} className="ritual-panel__form">
            <div className="ritual-panel__header">RENAME FILE</div>
            <div className="ritual-panel__body">
              <div className="ritual-divider" />
              <label className="ritual-field">
                <span className="ritual-label">NEW NAME</span>
                <input
                  className="ritual-input"
                  type="text"
                  value={renameModal.value}
                  onChange={e => setRenameModal(m => m ? { ...m, value: e.target.value } : null)}
                  autoFocus
                />
              </label>
              <div className="ritual-actions">
                <button type="submit" className="ritual-btn ritual-btn--primary">RENAME</button>
                <button type="button" className="ritual-btn ritual-btn--secondary" onClick={() => setRenameModal(null)}>CANCEL</button>
              </div>
            </div>
          </form>
        </div>
      </div>
    )}
    </>
  )
}

function PanelEmpty({ title, detail }: { title: string; detail: string }) {
  return (
    <div className="panel-empty">
      <span aria-hidden="true">—</span>
      <strong>{title}</strong>
      <small>{detail}</small>
    </div>
  )
}

// ── User/device grouping ──────────────────────────────────────────────────────

interface DeviceView {
  ambientAgents: string[]
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
      ambientAgents: m.ambient_agents ?? [],
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

// ── File icons ────────────────────────────────────────────────────────────────

const SvgIcon = ({ d, d2 }: { d: string; d2?: string }) => (
  <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="square" className="shrink-0" aria-hidden="true">
    <path d={d} />
    {d2 && <path d={d2} />}
  </svg>
)

function FileIcon({ name, isDir, isOpen }: { name: string; isDir: boolean; isOpen: boolean }) {
  if (isDir) {
    return isOpen
      ? <SvgIcon d="M1 4.5h14v9H1zM1 4.5l2-3h5l1.5 1.5" />
      : <SvgIcon d="M1 4.5h14v9H1zM1 4.5l2-3h4.5l1.5 1.5" />
  }
  const ext = name.includes('.') ? name.split('.').pop()!.toLowerCase() : ''
  // Code / markup
  if (['ts','tsx','js','jsx','mjs','cjs'].includes(ext))
    return <SvgIcon d="M2 2h8l4 4v8H2z" d2="M10 2v4h4M5.5 9.5l-2 1.5 2 1.5M10.5 9.5l2 1.5-2 1.5" />
  if (['rs','go','py','rb','java','c','cpp','h','cs','swift','kt'].includes(ext))
    return <SvgIcon d="M2 2h8l4 4v8H2z" d2="M10 2v4h4M5 8.5h6M5 11.5h4" />
  if (['html','htm','xml','svg','vue'].includes(ext))
    return <SvgIcon d="M2 2h8l4 4v8H2z" d2="M10 2v4h4M5 8l-2 2 2 2M11 8l2 2-2 2" />
  if (['css','scss','sass','less'].includes(ext))
    return <SvgIcon d="M2 2h8l4 4v8H2z" d2="M10 2v4h4M5 8.5c0-1 1.5-1.5 3-0.5s3 0.5 3-0.5" />
  // Data / config
  if (['json','jsonc','json5'].includes(ext))
    return <SvgIcon d="M2 2h8l4 4v8H2z" d2="M10 2v4h4M6 8l-1.5 2 1.5 2M10 8l1.5 2-1.5 2M8 7v2" />
  if (['toml','yaml','yml','env','ini','cfg','conf'].includes(ext))
    return <SvgIcon d="M2 2h8l4 4v8H2z" d2="M10 2v4h4M5 7.5h6M5 10h4M5 12.5h5" />
  // Docs
  if (['md','mdx','txt','rst','adoc'].includes(ext))
    return <SvgIcon d="M2 2h8l4 4v8H2z" d2="M10 2v4h4M5 8h6M5 10.5h6M5 13h4" />
  if (['pdf'].includes(ext))
    return <SvgIcon d="M2 2h8l4 4v8H2z" d2="M10 2v4h4M5 8.5c0 1.5 2 1.5 2 0V8M9 8v4M11 8h2" />
  // Images
  if (['png','jpg','jpeg','gif','webp','ico','bmp','avif'].includes(ext))
    return <SvgIcon d="M2 2h12v12H2zM2 10l3.5-3.5 3 3 2-2 3.5 3.5" d2="M10.5 5.5a1 1 0 1 1 0 .001" />
  // Lock / key files
  if (['pem','key','crt','cer','p12','pfx'].includes(ext))
    return <SvgIcon d="M5 7V5a3 3 0 0 1 6 0v2h1v6H4V7zM8 10v1.5" />
  // Generic fallback — plain document
  return <SvgIcon d="M2 1.5h8l4 4v9H2z" d2="M10 1.5v4h4" />
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

function FileTree({ nodes, onSelect, onOpen, onRename, onDelete, openMenu, onOpenMenu, selected, opened, openFolders, onToggleFolder, depth }: {
  nodes: TreeNode[]
  onSelect: (path: string) => void
  onOpen: (path: string) => void
  onRename: (path: string) => void
  onDelete: (path: string) => void
  openMenu: string | null
  onOpenMenu: (path: string | null) => void
  selected: string | null
  opened: string | null
  openFolders: Set<string>
  onToggleFolder: (path: string) => void
  depth: number
}) {
  return (
    <>
      {nodes.map(n => (
        <div key={n.path}>
          <div
            className={`file-row${selected === n.path ? ' selected' : ''}${opened === n.path ? ' is-open-center' : ''}`}
            style={{ paddingLeft: `${depth * 12}px` }}
          >
            <button
              className="file-name"
              onClick={() => {
                if (n.isDir) onToggleFolder(n.path)
                else { onOpenMenu(null); onSelect(n.path) }
              }}
              onDoubleClick={() => { if (!n.isDir) onOpen(n.path) }}
              title={n.path}
            >
              <FileIcon name={n.name} isDir={n.isDir} isOpen={openFolders.has(n.path)} />
              <span>{n.name}</span>
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
                {openMenu === n.path && (
                  <div className="file-menu file-menu-inline">
                    <button className="file-menu-item" onClick={() => onRename(n.path)}>Rename</button>
                    <button className="file-menu-item" onClick={() => onDelete(n.path)}>Delete</button>
                  </div>
                )}
              </span>
            )}
          </div>
          {n.isDir && openFolders.has(n.path) && (
            <FileTree
              nodes={n.children}
              onSelect={onSelect}
              onOpen={onOpen}
              onRename={onRename}
              onDelete={onDelete}
              openMenu={openMenu}
              onOpenMenu={onOpenMenu}
              selected={selected}
              opened={opened}
              openFolders={openFolders}
              onToggleFolder={onToggleFolder}
              depth={depth + 1}
            />
          )}
        </div>
      ))}
    </>
  )
}
