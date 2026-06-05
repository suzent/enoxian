import { useEffect, useRef, useState } from 'react'
import { EditorState } from '@codemirror/state'
import { EditorView, keymap, lineNumbers, highlightActiveLineGutter } from '@codemirror/view'
import { defaultKeymap, indentWithTab } from '@codemirror/commands'
import { javascript } from '@codemirror/lang-javascript'
import { python } from '@codemirror/lang-python'
import { rust } from '@codemirror/lang-rust'
import { markdown } from '@codemirror/lang-markdown'
import { json } from '@codemirror/lang-json'
import * as Y from 'yjs'
import { yCollab } from 'y-codemirror.next'
import { wsYjsUrl, getMembers } from '../api'
import { useApp } from '../context/AppContext'
import { YjsProvider, type YjsConnectionStatus } from '../lib/YjsProvider'
import { agentColor, agentColorLight } from '../lib/agentColor'
import { constrainCursorLabels } from '../lib/constrainCursorLabels'
import { peerLabel, shortenAgentId } from '../lib/displayName'

interface Props {
  filePath: string | null
  onBack?: () => void
}

function langExt(path: string) {
  const ext = path.split('.').pop() ?? ''
  switch (ext) {
    case 'js': case 'ts': case 'jsx': case 'tsx': return javascript({ typescript: ext === 'ts' || ext === 'tsx' })
    case 'py': return python()
    case 'rs': return rust()
    case 'md': return markdown()
    case 'json': return json()
    default: return []
  }
}

const enochTheme = EditorView.theme({
  '&': {
    backgroundColor: 'transparent',
    fontFamily: "'JetBrains Mono', monospace",
    fontSize: '20px',
    color: '#111111',
  },
  '.cm-content': { caretColor: '#111111', padding: '24px 32px', lineHeight: '1.75' },
  '.cm-cursor': { borderLeftColor: '#111111', borderLeftWidth: '2px' },
  '.cm-gutters': { backgroundColor: 'rgba(234,234,228,0.5)', borderRight: '1px dashed rgba(17,17,17,0.2)', color: '#555555' },
  '.cm-activeLineGutter': { backgroundColor: 'rgba(17,17,17,0.05)' },
  '.cm-activeLine': { backgroundColor: 'rgba(17,17,17,0.03)' },
  '.cm-selectionBackground': { backgroundColor: 'rgba(17,17,17,0.12) !important' },
  '&.cm-focused .cm-selectionBackground': { backgroundColor: 'rgba(17,17,17,0.15) !important' },
  '.cm-scroller': { overflow: 'auto' },
  '.cm-ySelection': { opacity: '1' },
  '.cm-yLineSelection': { opacity: '1' },
  '.cm-ySelectionCaret': {
    position: 'relative',
    borderLeft: '2px solid',
    borderRight: 'none',
    marginLeft: '-1px',
    marginRight: '-1px',
    boxSizing: 'border-box',
    display: 'inline',
    zIndex: '10',
  },
  // Initial position — constrainCursorLabels plugin overrides left/transform at runtime.
  '.cm-ySelectionInfo': {
    position: 'absolute',
    display: 'inline-block',
    top: '0',
    left: '0',
    fontSize: '11px',
    fontFamily: "'JetBrains Mono', monospace",
    fontStyle: 'normal',
    fontWeight: 'bold',
    lineHeight: 'normal',
    userSelect: 'none',
    pointerEvents: 'none',
    color: '#f4f3ef',
    padding: '2px 5px',
    whiteSpace: 'nowrap',
    borderRadius: '0',
    opacity: '1',
    transition: 'none',
    zIndex: '200',
  },
}, { dark: false })

function fileName(path: string) {
  return path.split('/').pop() || path
}

export default function EditorPanel({ filePath, onBack }: Props) {
  const { activeCircleId, status } = useApp()
  const editorRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<EditorView | null>(null)
  const providerRef = useRef<YjsProvider | null>(null)
  const ydocRef = useRef<Y.Doc | null>(null)
  const [connectionStatus, setConnectionStatus] = useState<YjsConnectionStatus>('disconnected')
  const [displayName, setDisplayName] = useState<string>(() => status?.agent_id ?? 'unknown')

  useEffect(() => {
    if (!activeCircleId || !status?.agent_id) return
    getMembers(activeCircleId)
      .then(members => {
        const me = members.find(m => m.agent_id === status.agent_id)
        if (!me) { setDisplayName(status.agent_id); return }
        const owner = peerLabel(me.owner, me.agent_id)
        const device = me.device_label || shortenAgentId(me.agent_id)
        setDisplayName(owner === device ? owner : `${owner} · ${device}`)
      })
      .catch(() => {})
  }, [activeCircleId, status?.agent_id])

  useEffect(() => {
    if (!editorRef.current || !filePath || !activeCircleId) return

    viewRef.current?.destroy()
    providerRef.current?.destroy()
    ydocRef.current?.destroy()
    setConnectionStatus('connecting')

    const ydoc = new Y.Doc()
    ydocRef.current = ydoc
    const ytext = ydoc.getText(filePath)

    const url = wsYjsUrl(activeCircleId, filePath)
    const provider = new YjsProvider(
      url,
      ydoc,
      () => setConnectionStatus('synced'),
      setConnectionStatus,
    )
    providerRef.current = provider

    const awareness = provider.awareness
    awareness.setLocalStateField('user', {
      name: displayName,
      color: agentColor(status?.agent_id ?? ''),
      colorLight: agentColorLight(status?.agent_id ?? ''),
    })

    const state = EditorState.create({
      extensions: [
        keymap.of([...defaultKeymap, indentWithTab]),
        lineNumbers(),
        highlightActiveLineGutter(),
        langExt(filePath),
        enochTheme,
        EditorView.lineWrapping,
        yCollab(ytext, awareness),
        constrainCursorLabels,
      ],
    })

    const view = new EditorView({ state, parent: editorRef.current })
    viewRef.current = view

    return () => {
      view.destroy()
      provider.destroy()
      ydoc.destroy()
    }
  }, [filePath, activeCircleId, status?.agent_id, displayName])

  if (!filePath) {
    return (
      <main className="app-editor-panel flex min-h-0 items-center justify-center z-10 bg-transparent p-4">
        <div className="border-2 border-obsidian bg-alabaster p-10 font-mono text-[11px] text-slate
                        shadow-[12px_12px_0px_rgba(17,17,17,0.12)] max-w-sm w-full text-center">
          <div className="text-obsidian font-bold mb-2 text-[13px]">NO ARTIFACT SELECTED</div>
          <div>Select a file from the Artifact Filesystem to begin editing.</div>
        </div>
      </main>
    )
  }

  return (
    <main className="app-editor-panel flex min-h-0 flex-col z-10 bg-transparent overflow-hidden">
      <div className="editor-frame ide-frame w-full flex min-h-0 flex-col">
        <div className="section-header editor-titlebar">
          <span>Editor</span>
        </div>
        <div className="ide-topbar">
          <button onClick={onBack} className="ide-back" title="Back to circle chat">
            BACK TO CHAT
          </button>
          <div className="ide-file-meta min-w-0">
            <div className="ide-file-name truncate" title={filePath}>{fileName(filePath)}</div>
            <div className="ide-file-path truncate" title={filePath}>{filePath}</div>
          </div>
          <span className={`ide-sync ${connectionStatus}`}>
            {connectionStatus === 'synced' ? 'SYNCED' : connectionStatus === 'connecting' ? 'CONNECTING' : 'OFFLINE'}
          </span>
        </div>
        <div className="ide-subbar">
          <span>collaborative document</span>
          <span>{status?.agent_id ?? 'unknown'}</span>
        </div>
        <div ref={editorRef} className="ide-editor min-h-0 flex-1 overflow-auto bg-transparent" />
      </div>
    </main>
  )
}
