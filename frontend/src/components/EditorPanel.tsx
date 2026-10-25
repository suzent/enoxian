import { useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
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
import { renderMarkdown } from '../lib/markdown'
import { Maximize2, X, ChevronLeft } from 'lucide-react'
import SegmentedTabs, { type SegmentedTabOption } from './ui/SegmentedTabs'

export type FileViewMode = 'source' | 'preview'

const PREVIEW_FIRST_VIEWS: readonly SegmentedTabOption<FileViewMode>[] = [
  { value: 'preview', content: 'PREVIEW' },
  { value: 'source', content: 'SOURCE' },
]

interface Props {
  filePath: string | null
  onBack?: () => void
  initialMode?: FileViewMode
}

interface FileQuickViewProps {
  filePath: string
  onOpen: (mode: FileViewMode) => void
  onClose: () => void
  full?: boolean
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
    fontSize: '14px',
    color: '#111111',
  },
  '.cm-content': { caretColor: '#111111', padding: '20px 24px 40px', lineHeight: '1.65' },
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
  // Label: hidden by default, flashed in by the constrainCursorLabels plugin on
  // each cursor move (adds .cm-ySelectionInfo--active which fades out after 2s),
  // and always shown on hover via the rule below.
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
    opacity: '0',
    transition: 'opacity 0.3s ease',
    zIndex: '200',
  },
  '.cm-ySelectionInfo.cm-ySelectionInfo--active': {
    opacity: '1',
    transition: 'none',
  },
  '.cm-ySelectionCaret:hover .cm-ySelectionInfo': {
    opacity: '1',
    transition: 'opacity 0.15s ease',
  },
}, { dark: false })

function fileName(path: string) {
  return path.split('/').pop() || path
}

type PreviewKind = 'markdown' | 'html' | null

function previewKind(path: string): PreviewKind {
  const ext = path.split('.').pop()?.toLowerCase()
  if (ext === 'md' || ext === 'markdown') return 'markdown'
  if (ext === 'html' || ext === 'htm') return 'html'
  return null
}

function renderHtmlDocument(source: string) {
  const doc = new DOMParser().parseFromString(source, 'text/html')
  const policy = doc.createElement('meta')
  policy.httpEquiv = 'Content-Security-Policy'
  policy.content = "default-src 'none'; img-src data: blob:; style-src 'unsafe-inline'; font-src data:; media-src data: blob:;"
  doc.head.prepend(policy)
  return `<!doctype html>\n${doc.documentElement.outerHTML}`
}

function connectionLabel(status: YjsConnectionStatus) {
  if (status === 'synced') return 'LIVE'
  if (status === 'connecting') return 'CONNECTING'
  return 'OFFLINE'
}

interface FileSurfaceHeaderProps {
  filePath: string
  connectionStatus: YjsConnectionStatus
  onBack?: () => void
  closeLabel?: string
  backLabel?: string
}

function FileSurfaceHeader({ filePath, connectionStatus, onBack, closeLabel = 'Close file', backLabel }: FileSurfaceHeaderProps) {
  return (
    <div className="file-surface-header">
      {onBack && backLabel && <button type="button" className="file-surface-return" onClick={onBack} aria-label={closeLabel} title={closeLabel}>
        <ChevronLeft size={18} aria-hidden="true" />
      </button>}
      <div className="file-surface-header__meta">
        <strong title={filePath}>{fileName(filePath)}</strong>
        {filePath !== fileName(filePath) && <span title={filePath}>{filePath}</span>}
      </div>
      <span className={`file-surface-header__status ${connectionStatus}`}>
        {connectionLabel(connectionStatus)}
      </span>
      {onBack && !backLabel && (
        <button type="button" className="file-surface-header__back" onClick={onBack} aria-label={closeLabel} title={closeLabel}>
          <X size={15} aria-hidden="true" />
        </button>
      )}
    </div>
  )
}

interface FileSurfaceControlsProps {
  kind: PreviewKind
  value: FileViewMode
  onChange: (value: FileViewMode) => void
  ariaLabel: string
  action?: ReactNode
}

function FileSurfaceControls({ kind, value, onChange, ariaLabel, action }: FileSurfaceControlsProps) {
  return (
    <div className="file-surface-controls">
      {kind ? (
        <SegmentedTabs
          className="file-surface-modes"
          ariaLabel={ariaLabel}
          value={value}
          onChange={onChange}
          options={PREVIEW_FIRST_VIEWS}
        />
      ) : <span className="file-surface-type">SOURCE</span>}
      {action}
    </div>
  )
}

function DocumentPreview({ kind, source, filePath, compact = false }: {
  kind: PreviewKind; source: string; filePath: string; compact?: boolean
}) {
  const rendered = useMemo(() => kind === 'markdown' ? renderMarkdown(source) : kind === 'html' ? renderHtmlDocument(source) : '', [kind, source])
  if (kind === 'markdown') return <article className={`markdown-preview${compact ? ' markdown-preview--compact' : ''}`} dangerouslySetInnerHTML={{ __html: rendered }} />
  if (kind === 'html') return <div className="document-html-preview">
    <div className="html-preview-notice">Sandboxed preview · scripts and network disabled</div>
    <iframe title={`Preview of ${fileName(filePath)}`} sandbox="" srcDoc={rendered} />
  </div>
  return null
}

export function FileQuickView({ filePath, onOpen, onClose, full = false }: FileQuickViewProps) {
  const { activeCircleId } = useApp()
  const [documentText, setDocumentText] = useState('')
  const [connectionStatus, setConnectionStatus] = useState<YjsConnectionStatus>('connecting')
  const kind = previewKind(filePath)
  const [viewMode, setViewMode] = useState<FileViewMode>(kind ? 'preview' : 'source')

  useEffect(() => {
    setDocumentText('')
    setViewMode(previewKind(filePath) ? 'preview' : 'source')
  }, [filePath])

  useEffect(() => {
    if (!activeCircleId) return
    setConnectionStatus('connecting')
    const ydoc = new Y.Doc()
    const ytext = ydoc.getText(filePath)
    const updateText = () => setDocumentText(ytext.toString())
    ytext.observe(updateText)
    updateText()

    const provider = new YjsProvider(
      wsYjsUrl(activeCircleId, filePath),
      ydoc,
      () => setConnectionStatus('synced'),
      setConnectionStatus,
    )

    return () => {
      ytext.unobserve(updateText)
      provider.destroy()
      ydoc.destroy()
    }
  }, [activeCircleId, filePath])

  return (
    <section className={`file-quick-view${full ? ' file-quick-view--full' : ''}`} aria-label={`Preview of ${fileName(filePath)}`}>
      <FileSurfaceHeader
        filePath={filePath}
        connectionStatus={connectionStatus}
        onBack={onClose}
        closeLabel="Back to files"
        backLabel="Files"
      />
      <FileSurfaceControls
        kind={kind}
        value={viewMode}
        onChange={setViewMode}
        ariaLabel="Sidebar file view"
        action={<button type="button" className="file-surface-action" onClick={() => onOpen(viewMode)} title="Open in center editor">
          <Maximize2 size={13} aria-hidden="true" />
          <span>Open in center</span>
        </button>}
      />
      <div className="file-quick-view__body">
        {viewMode === 'source' && <pre className="file-quick-view__source">{documentText}</pre>}
        {viewMode === 'preview' && <DocumentPreview kind={kind} source={documentText} filePath={filePath} compact />}
      </div>
    </section>
  )
}

export default function EditorPanel({ filePath, onBack, initialMode = 'source' }: Props) {
  const { activeCircleId, status } = useApp()
  const editorRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<EditorView | null>(null)
  const providerRef = useRef<YjsProvider | null>(null)
  const ydocRef = useRef<Y.Doc | null>(null)
  const [connectionStatus, setConnectionStatus] = useState<YjsConnectionStatus>('disconnected')
  const [displayName, setDisplayName] = useState<string>(() => status?.agent_id ?? 'unknown')
  const [viewMode, setViewMode] = useState<FileViewMode>(initialMode)
  const [documentText, setDocumentText] = useState('')
  const kind = filePath ? previewKind(filePath) : null

  useEffect(() => {
    setViewMode(initialMode)
  }, [filePath, initialMode])

  useEffect(() => { setDocumentText('') }, [filePath])

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
    const updatePreview = () => setDocumentText(ytext.toString())
    ytext.observe(updatePreview)
    updatePreview()

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
      ytext.unobserve(updatePreview)
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
        <FileSurfaceHeader
          filePath={filePath}
          connectionStatus={connectionStatus}
          onBack={onBack}
          closeLabel="Close editor and return to chat"
        />
        <FileSurfaceControls
          kind={kind}
          value={viewMode}
          onChange={setViewMode}
          ariaLabel="File view"
        />
        <div className={`ide-document min-h-0 flex-1${viewMode === 'preview' && kind ? ' is-previewing' : ''}`}>
          <div ref={editorRef} className="ide-editor min-h-0 h-full overflow-auto bg-transparent" />
          {viewMode === 'preview' && kind && (
            <div className={`file-preview file-preview--${kind}`}>
              <DocumentPreview kind={kind} source={documentText} filePath={filePath} />
            </div>
          )}
        </div>
      </div>
    </main>
  )
}
