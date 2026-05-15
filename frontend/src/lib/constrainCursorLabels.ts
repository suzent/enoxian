import { EditorView, ViewPlugin, ViewUpdate } from '@codemirror/view'

/**
 * Repositions `.cm-ySelectionInfo` labels so they stay within the editor's
 * visible scroll area regardless of where the cursor caret sits.
 *
 * y-codemirror.next places carets as inline widgets; their labels are
 * `position:absolute` children. Because the scroller clips painted content
 * that goes outside its bounds, labels near the left/right/top edges get
 * truncated. This plugin runs after each render and clamps each label's
 * horizontal position so it never overflows.
 */
export const constrainCursorLabels = ViewPlugin.fromClass(class {
  constructor(private view: EditorView) {
    this.reposition()
  }

  update(update: ViewUpdate) {
    if (update.docChanged || update.viewportChanged || update.geometryChanged) {
      this.reposition()
    }
  }

  reposition() {
    const scroller = this.view.scrollDOM
    const scrollerRect = scroller.getBoundingClientRect()

    const labels = scroller.querySelectorAll<HTMLElement>('.cm-ySelectionInfo')
    labels.forEach(label => {
      // Reset any previous override so getBoundingClientRect is accurate.
      label.style.left = ''
      label.style.transform = ''

      const caret = label.parentElement
      if (!caret) return

      const caretRect = caret.getBoundingClientRect()
      const labelWidth = label.offsetWidth

      // Ideal: label's right edge aligns with caret left edge (appears to the left of caret).
      // This prevents right-edge overflow when the cursor is at the end of a line.
      let x = caretRect.left - scrollerRect.left - labelWidth

      // Clamp: don't go past left edge of scroller content area.
      const contentLeft = this.view.contentDOM.getBoundingClientRect().left - scrollerRect.left
      if (x < contentLeft) x = contentLeft

      // Clamp: don't go past right edge of visible scroller.
      const maxX = scrollerRect.width - labelWidth
      if (x > maxX) x = maxX

      label.style.left = `${x}px`
      label.style.transform = 'translateY(-100%)'
    })
  }
})
