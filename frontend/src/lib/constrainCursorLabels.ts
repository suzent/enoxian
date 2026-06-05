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
      // Reset any previous override so getBoundingClientRect is accurate.
      label.style.left = ''
      label.style.transform = ''

      const caret = label.parentElement
      if (!caret) return

      const caretRect = caret.getBoundingClientRect()
      const labelWidth = label.offsetWidth
      const labelHeight = label.offsetHeight

      const caretX = caretRect.left - scrollerRect.left
      const caretY = caretRect.top - scrollerRect.top

      // Flip below the caret when there isn't enough room above. Safari clips
      // overflow above the scroller top and offsetHeight can be 0 before layout,
      // so use lineHeight as a conservative stand-in for label height.
      const estimatedLabelHeight = Math.max(labelHeight, lineHeight)
      const fitsAbove = caretY - estimatedLabelHeight >= 0
      if (fitsAbove) {
        label.style.transform = 'translateY(-100%)'
      } else {
        label.style.transform = `translateY(${lineHeight}px)`
      }

      // Ideal viewport position: label's right edge aligns with the caret.
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
