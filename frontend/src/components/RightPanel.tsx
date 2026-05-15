import { useState, useEffect } from 'react'
import type { Presence, Task } from '../types'
import { getWho, getTasks, createTask, claimTask, doneTask, getFiles } from '../api'
import { useApp } from '../context/AppContext'

interface Props {
  onFileSelect: (path: string) => void
  selectedFile: string | null
}

function age(isoStr: string) {
  const secs = Math.floor((Date.now() - new Date(isoStr).getTime()) / 1000)
  if (secs < 60) return 'just now'
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`
  return `${Math.floor(secs / 3600)}h ago`
}

export default function RightPanel({ onFileSelect, selectedFile }: Props) {
  const { activeCircleId, status } = useApp()
  const [presence, setPresence] = useState<Presence[]>([])
  const [tasks, setTasks] = useState<Task[]>([])
  const [files, setFiles] = useState<string[]>([])
  const [newTaskTitle, setNewTaskTitle] = useState('')
  const [newTaskDesc, setNewTaskDesc] = useState('')
  const [creating, setCreating] = useState(false)

  useEffect(() => {
    setPresence([])
    setTasks([])
    setFiles([])
    if (!activeCircleId) return

    let cancelled = false

    const refresh = () => {
      getWho(activeCircleId).then(data => { if (!cancelled) setPresence(data) }).catch(() => {})
      getTasks(activeCircleId).then(data => { if (!cancelled) setTasks(data) }).catch(() => {})
      getFiles(activeCircleId).then(data => { if (!cancelled) setFiles(data) }).catch(e => console.error('[files]', e))
    }

    refresh()
    const id = setInterval(refresh, 15_000)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [activeCircleId])

  const refreshTasks = () => {
    if (activeCircleId) getTasks(activeCircleId).then(setTasks).catch(() => {})
  }

  const submitTask = () => {
    const title = newTaskTitle.trim()
    if (!title || !activeCircleId || !status) return
    createTask(activeCircleId, title, newTaskDesc.trim(), status.agent_id)
      .then(() => { setNewTaskTitle(''); setNewTaskDesc(''); setCreating(false); refreshTasks() })
      .catch(() => {})
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

  return (
    <aside className="flex flex-col border-l-2 border-obsidian bg-alabaster/85 z-10 overflow-hidden">

      {/* ── Presence ─────────────────────────────────────────────────────── */}
      <div className="section-header">Structural Entities</div>
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
          <button onClick={submitTask} className="enoch-btn self-start">CREATE</button>
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
  const dot = p.status === 'online' ? '●' : p.status === 'idle' ? '◑' : '○'
  return (
    <div className="flex justify-between items-baseline">
      <span className="font-bold">@{p.agent_id}</span>
      <div className="flex gap-2 items-baseline">
        <span className="text-[9px] font-bold">{dot} {p.status.toUpperCase()}</span>
        <span className="text-[9px] text-slate">{age(p.last_seen)}</span>
      </div>
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
                        hover:bg-obsidian/5 ${selected === n.path ? 'bg-obsidian text-alabaster' : ''}`}
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
