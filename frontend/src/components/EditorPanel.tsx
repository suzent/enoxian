import { useEffect, useRef, useState } from 'react'
import { EditorState } from '@codemirror/state'
import { EditorView, keymap } from '@codemirror/view'
import { defaultKeymap, indentWithTab } from '@codemirror/commands'
import { javascript } from '@codemirror/lang-javascript'
import { python } from '@codemirror/lang-python'
import { rust } from '@codemirror/lang-rust'
import { markdown } from '@codemirror/lang-markdown'
import { json } from '@codemirror/lang-json'
import * as Y from 'yjs'
import { yCollab } from 'y-codemirror.next'
import { wsYjsUrl } from '../api'
import { useApp } from '../context/AppContext'
import { YjsProvider } from '../lib/YjsProvider'

// Deterministic palette — same agent always gets same color across sessions.
const CURSOR_COLORS = ['#c0392b','#2980b9','#27ae60','#8e44ad','#d35400','#16a085','#c0392b']
function agentColor(id: string): string {
  let h = 0
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0
  return CURSOR_COLORS[h % CURSOR_COLORS.length]
}
function agentColorLight(id: string): string {
  return agentColor(id) + '33' // 20% alpha
}

interface Props {
  filePath: string | null
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
    fontSize: '12px',
    color: '#111111',
  },
  '.cm-content': { caretColor: '#111111', padding: '20px 24px', lineHeight: '1.7' },
  '.cm-cursor': { borderLeftColor: '#111111', borderLeftWidth: '2px' },
  '.cm-gutters': { backgroundColor: 'rgba(234,234,228,0.5)', borderRight: '1px dashed rgba(17,17,17,0.2)', color: '#555555' },
  '.cm-activeLineGutter': { backgroundColor: 'rgba(17,17,17,0.05)' },
  '.cm-activeLine': { backgroundColor: 'rgba(17,17,17,0.03)' },
  '.cm-selectionBackground': { backgroundColor: 'rgba(17,17,17,0.12) !important' },
  '&.cm-focused .cm-selectionBackground': { backgroundColor: 'rgba(17,17,17,0.15) !important' },
  '.cm-scroller': { overflow: 'auto' },
  // Remote cursor caret
  '.cm-ySelectionCaret': {
    position: 'relative',
    borderLeft: '2px solid',
    borderRight: 'none',
    marginLeft: '-1px',
    marginRight: '-1px',
    boxSizing: 'border-box',
    display: 'inline',
  },
  // Remote selection highlight
  '.cm-ySelection': { opacity: '0.25' },
  // Name label above cursor
  '.cm-ySelectionInfo': {
    position: 'absolute',
    top: '-1.4em',
    left: '-1px',
    fontSize: '10px',
    fontFamily: "'JetBrains Mono', monospace",
    fontStyle: 'normal',
    fontWeight: 'bold',
    lineHeight: 'normal',
    userSelect: 'none',
    color: '#eaeae4',
    padding: '1px 4px',
    whiteSpace: 'nowrap',
    borderRadius: '0',
  },
}, { dark: false })

export default function EditorPanel({ filePath }: Props) {
  const { activeCircleId, status } = useApp()
  const editorRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<EditorView | null>(null)
  const providerRef = useRef<YjsProvider | null>(null)
  const ydocRef = useRef<Y.Doc | null>(null)
  const [synced, setSynced] = useState(false)

  useEffect(() => {
    if (!editorRef.current || !filePath || !activeCircleId) return

    // Tear down previous instance
    viewRef.current?.destroy()
    providerRef.current?.destroy()
    ydocRef.current?.destroy()
    setSynced(false)

    const ydoc = new Y.Doc()
    ydocRef.current = ydoc
    const ytext = ydoc.getText(filePath)

    const url = wsYjsUrl(activeCircleId, filePath)
    const provider = new YjsProvider(url, ydoc, () => setSynced(true))
    providerRef.current = provider

    const awareness = provider.awareness
    awareness.setLocalStateField('user', {
      name: status?.agent_id ?? 'unknown',
      color: agentColor(status?.agent_id ?? ''),
      colorLight: agentColorLight(status?.agent_id ?? ''),
    })

    const state = EditorState.create({
      extensions: [
        keymap.of([...defaultKeymap, indentWithTab]),
        langExt(filePath),
        enochTheme,
        EditorView.lineWrapping,
        yCollab(ytext, awareness),
      ],
    })

    const view = new EditorView({ state, parent: editorRef.current })
    viewRef.current = view

    return () => {
      view.destroy()
      provider.destroy()
      ydoc.destroy()
    }
  }, [filePath, activeCircleId, status?.agent_id])

  if (!filePath) {
    return (
      <main className="flex items-center justify-center z-10 bg-transparent">
        <div className="border-2 border-obsidian bg-alabaster p-10 font-mono text-[11px] text-slate
                        shadow-[12px_12px_0px_rgba(17,17,17,0.12)] max-w-sm w-full text-center">
          <div className="text-obsidian font-bold mb-2 text-[13px]">NO ARTIFACT SELECTED</div>
          <div>Select a file from the Artifact Filesystem to begin editing.</div>
        </div>
      </main>
    )
  }

  return (
    <main className="flex flex-col items-center justify-center z-10 bg-transparent p-8 overflow-hidden">
      <div className="border-2 border-obsidian bg-alabaster w-full max-w-3xl flex flex-col
                      shadow-[20px_20px_0px_rgba(17,17,17,0.13)]"
           style={{ maxHeight: 'calc(100vh - 140px)' }}>
        <div className="flex justify-between items-center px-4 py-3 border-b-2 border-obsidian
                        font-mono text-[10px] font-bold uppercase bg-alabaster">
          <span>{filePath}</span>
          <span className={`text-[9px] ${synced ? 'text-obsidian' : 'text-slate'}`}>
            {synced ? '◉ SYNCED' : '◎ CONNECTING...'}
          </span>
        </div>
        <div ref={editorRef} className="flex-1 overflow-auto bg-transparent" />
      </div>
    </main>
  )
}
