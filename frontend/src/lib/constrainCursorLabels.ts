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

      const caretX = caretRect.left - scrollerRect.left

      // Ideal viewport position: label's right edge aligns with the caret.
      // The label itself is absolutely positioned inside the caret widget, so
      // we clamp in scroller coordinates and convert back to caret-local left.
      let viewportX = caretX - labelWidth

      // Clamp: don't go past left edge of scroller content area.
      const contentLeft = this.view.contentDOM.getBoundingClientRect().left - scrollerRect.left
      if (viewportX < contentLeft) viewportX = contentLeft

      // Clamp: don't go past right edge of visible scroller.
      const maxX = scrollerRect.width - labelWidth
      if (viewportX > maxX) viewportX = maxX

      label.style.left = `${viewportX - caretX}px`
      label.style.transform = 'translateY(-100%)'
    })
  }
})
