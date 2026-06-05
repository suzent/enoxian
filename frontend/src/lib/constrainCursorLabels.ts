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
const ACTIVE_CLASS = 'cm-ySelectionInfo--active'
const LABEL_FADE_MS = 2000

export const constrainCursorLabels = ViewPlugin.fromClass(class {
  private lastSignature = ''
  private repaintFrame = 0
  private fadeTimers = new Map<HTMLElement, ReturnType<typeof setTimeout>>()

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
    for (const t of this.fadeTimers.values()) clearTimeout(t)
  }

  queueReposition() {
    if (this.repaintFrame) cancelAnimationFrame(this.repaintFrame)
    this.repaintFrame = requestAnimationFrame(() => {
      this.repaintFrame = 0
      this.reposition()
    })
  }

  flashLabel(label: HTMLElement) {
    label.classList.add(ACTIVE_CLASS)
    const existing = this.fadeTimers.get(label)
    if (existing) clearTimeout(existing)
    const t = setTimeout(() => {
      label.classList.remove(ACTIVE_CLASS)
      this.fadeTimers.delete(label)
    }, LABEL_FADE_MS)
    this.fadeTimers.set(label, t)
  }

  reposition() {
    const scroller = this.view.scrollDOM
    const scrollerRect = scroller.getBoundingClientRect()

    const labels = scroller.querySelectorAll<HTMLElement>('.cm-ySelectionInfo')

    // Flash labels when cursor positions change (signature = name+position).
    const signature = [...labels].map(label => {
      const caret = label.parentElement
      const rect = caret?.getBoundingClientRect()
      return `${label.textContent}@${rect?.left.toFixed(0)},${rect?.top.toFixed(0)}`
    }).join('|')
    if (signature !== this.lastSignature) {
      this.lastSignature = signature
      labels.forEach(label => this.flashLabel(label))
    }

    const lineHeight = this.view.defaultLineHeight

    labels.forEach(label => {
      const caret = label.parentElement
      if (!caret) return

      const caretRect = caret.getBoundingClientRect()
      const caretX = caretRect.left - scrollerRect.left
      const caretY = caretRect.top - scrollerRect.top

      label.style.left = ''
      const labelWidth = label.offsetWidth

      const fitsAbove = caretY >= lineHeight
      label.style.transform = fitsAbove
        ? 'translateY(-100%)'
        : `translateY(${lineHeight}px)`

      let viewportX = caretX - labelWidth
      const contentLeft = this.view.contentDOM.getBoundingClientRect().left - scrollerRect.left
      if (viewportX < contentLeft) viewportX = contentLeft
      const maxX = scrollerRect.width - labelWidth
      if (viewportX > maxX) viewportX = maxX

      label.style.left = `${viewportX - caretX}px`
    })
  }
})
