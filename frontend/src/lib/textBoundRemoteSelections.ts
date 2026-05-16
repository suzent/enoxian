import type { Range } from '@codemirror/state'
import { Decoration, type DecorationSet, EditorView, ViewPlugin, type ViewUpdate } from '@codemirror/view'
import * as Y from 'yjs'
import { ySyncFacet } from 'y-codemirror.next'

interface RemoteUser {
  color?: string
  colorLight?: string
}

interface RemoteCursor {
  anchor?: unknown
  head?: unknown
}

interface RemoteState {
  user?: RemoteUser
  cursor?: RemoteCursor | null
}

export const textBoundRemoteSelections = ViewPlugin.fromClass(class {
  decorations: DecorationSet = Decoration.none
  private awareness: any
  private listener: () => void

  constructor(private view: EditorView) {
    const conf = view.state.facet(ySyncFacet)
    this.awareness = conf.awareness
    this.listener = () => {
      this.decorations = this.buildDecorations()
      this.view.dispatch({})
    }
    this.awareness?.on('change', this.listener)
    this.decorations = this.buildDecorations()
  }

  update(update: ViewUpdate) {
    if (update.docChanged || update.viewportChanged) {
      this.decorations = this.buildDecorations()
    }
  }

  destroy() {
    this.awareness?.off('change', this.listener)
  }

  private buildDecorations(): DecorationSet {
    const conf = this.view.state.facet(ySyncFacet)
    const ytext = conf.ytext
    const ydoc = ytext.doc as Y.Doc
    const ranges: Array<Range<Decoration>> = []

    conf.awareness?.getStates().forEach((state: RemoteState, clientId: number) => {
      if (clientId === conf.awareness.doc.clientID) return

      const cursor = state.cursor
      if (!cursor?.anchor || !cursor?.head) return

      const anchor = Y.createAbsolutePositionFromRelativePosition(cursor.anchor as Y.RelativePosition, ydoc)
      const head = Y.createAbsolutePositionFromRelativePosition(cursor.head as Y.RelativePosition, ydoc)
      if (!anchor || !head || anchor.type !== ytext || head.type !== ytext) return

      const start = Math.min(anchor.index, head.index)
      const end = Math.max(anchor.index, head.index)
      if (start === end) return

      const color = state.user?.color ?? '#30bced'
      const colorLight = state.user?.colorLight ?? `${color}33`
      const selection = Decoration.mark({
        attributes: { style: `background-color: ${colorLight}` },
        class: 'cm-yTextRemoteSelection',
      })

      const startLine = this.view.state.doc.lineAt(start)
      const endLine = this.view.state.doc.lineAt(end)
      if (startLine.number === endLine.number) return

      for (let lineNo = startLine.number + 1; lineNo < endLine.number; lineNo++) {
        const line = this.view.state.doc.line(lineNo)
        if (line.from < line.to) {
          ranges.push(selection.range(line.from, line.to))
        }
      }
    })

    return Decoration.set(ranges, true)
  }
}, {
  decorations: value => value.decorations,
})
