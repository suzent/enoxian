import { EditorView, ViewPlugin, ViewUpdate } from '@codemirror/view'

/**
 * Repositions remote cursor labels so they stay within the editor's
 * visible scroll area regardless of where the cursor caret sits.
 *
 * The labels are `position:absolute` children of inline caret widgets. Because
 * the scroller clips painted content that goes outside its bounds, labels near
 * the left/right/top edges get truncated. This plugin runs after each render
 * and clamps each label's horizontal position so it never overflows.
 */
export const constrainCursorLabels = ViewPlugin.fromClass(class {
  private lastSignature = ''
  private repaintFrame = 0

  constructor(private view: EditorView) {
    this.reposition()
  }

  update(update: ViewUpdate) {
    this.reposition()
    if (update.docChanged || update.viewportChanged || update.geometryChanged) {
      this.queueReposition()
    }
  }

  destroy() {
    if (this.repaintFrame) cancelAnimationFrame(this.repaintFrame)
  }

  queueReposition() {
    if (this.repaintFrame) cancelAnimationFrame(this.repaintFrame)
    this.repaintFrame = requestAnimationFrame(() => {
      this.repaintFrame = 0
      this.reposition()
    })
  }

  forceSafariRepaint() {
    const scroller = this.view.scrollDOM
    scroller.style.transform = 'translateZ(0)'
    requestAnimationFrame(() => {
      if (scroller.style.transform === 'translateZ(0)') {
        scroller.style.transform = ''
      }
    })
  }

  reposition() {
    const scroller = this.view.scrollDOM
    const scrollerRect = scroller.getBoundingClientRect()

    const labels = scroller.querySelectorAll<HTMLElement>('.cm-ySelectionInfo')
    const signature = [...labels].map(label => label.textContent ?? '').join('|')
    if (signature !== this.lastSignature) {
      this.lastSignature = signature
      this.forceSafariRepaint()
    }

    const lineHeight = this.view.defaultLineHeight

    labels.forEach(label => {
      const caret = label.parentElement
      if (!caret) return

      // Measure caret position before touching the label's styles so that
      // getBoundingClientRect reflects the actual DOM layout.
      const caretRect = caret.getBoundingClientRect()
      const caretX = caretRect.left - scrollerRect.left
      const caretY = caretRect.top - scrollerRect.top

      // Reset horizontal override only — leave transform alone until we decide.
      label.style.left = ''

      const labelWidth = label.offsetWidth

      // Flip below when there isn't enough room above. Use lineHeight as the
      // label height estimate: offsetHeight can be 0 in Safari before first layout.
      const fitsAbove = caretY >= lineHeight
      label.style.transform = fitsAbove
        ? 'translateY(-100%)'
        : `translateY(${lineHeight}px)`

      // Ideal position: label's right edge aligns with the caret.
      let viewportX = caretX - labelWidth

      // Clamp: don't go past left edge of scroller content area.
      const contentLeft = this.view.contentDOM.getBoundingClientRect().left - scrollerRect.left
      if (viewportX < contentLeft) viewportX = contentLeft

      // Clamp: don't go past right edge of visible scroller.
      const maxX = scrollerRect.width - labelWidth
      if (viewportX > maxX) viewportX = maxX

      label.style.left = `${viewportX - caretX}px`
    })
  }
})
