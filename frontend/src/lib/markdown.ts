import { marked } from 'marked'
import DOMPurify from 'dompurify'

/**
 * Markdown → sanitized HTML, shared by the file preview and the chat
 * transcript. Both render text written elsewhere — a file synced from another
 * device, a message posted by another peer's agent — so neither may trust it.
 *
 * The output is a fragment meant for `dangerouslySetInnerHTML`. Everything that
 * makes that safe happens here: DOMPurify strips active content, remote images
 * are never fetched, and links cannot reach back into the app.
 */

// Belt and braces over DOMPurify's defaults: these are the tags whose whole
// point is to execute or to collect input, and neither surface wants them.
const FORBID_TAGS = ['script', 'iframe', 'object', 'embed', 'form', 'input', 'button']

// Rendered text may not reach into the app's own styling. `class` is the sharp
// one: the mention chip is how the UI says "the server really did register this
// as a ping", so a message able to write `class="mention-chip"` could forge one
// and impersonate a mention of anybody. `style` and `id` go for the same
// reason — neither is worth letting a remote document redecorate the page or
// collide with an element the app addresses by id.
const FORBID_ATTR = ['class', 'style', 'id']

// Tags whose text is quoted verbatim. An `@name` inside one of these is part of
// the snippet being shown — chipping it would rewrite someone's code sample.
const VERBATIM_TAGS = new Set(['CODE', 'PRE', 'KBD', 'SAMP'])

// `@` followed by mention-body characters (letters, digits, -, _, /).
const MENTION_RE = /@([A-Za-z0-9_\-/]+)/g

export interface MarkdownOptions {
  /** Treat a single newline as a line break (GFM `breaks`). Chat messages are
   *  written as plain text where a newline means a newline; a markdown file
   *  means paragraph flow, so it leaves this off. */
  breaks?: boolean
  /** The server-parsed mention list (owner/device/agent bodies) — the ground
   *  truth for "this registered as a mention". A `@token` in the text is
   *  chipped only if it appears here, so a typo that matched nobody stays
   *  plain text rather than pretending to be a ping. */
  mentions?: string[]
}

/** Wrap recognised `@mentions` in chips, in place, skipping verbatim text. */
function chipMentions(doc: Document, mentions: string[]) {
  const recognized = new Set(mentions)
  const walker = doc.createTreeWalker(doc.body, NodeFilter.SHOW_TEXT)
  // Collect first: replacing nodes while walking invalidates the walker.
  const targets: Text[] = []
  while (walker.nextNode()) {
    const node = walker.currentNode as Text
    if (!node.textContent?.includes('@')) continue
    let el: HTMLElement | null = node.parentElement
    let verbatim = false
    while (el && el !== doc.body) {
      if (VERBATIM_TAGS.has(el.tagName)) { verbatim = true; break }
      el = el.parentElement
    }
    if (!verbatim) targets.push(node)
  }

  for (const node of targets) {
    const text = node.textContent ?? ''
    const fragment = doc.createDocumentFragment()
    let last = 0
    let match: RegExpExecArray | null
    MENTION_RE.lastIndex = 0
    while ((match = MENTION_RE.exec(text)) !== null) {
      if (!recognized.has(match[1])) continue // unrecognised — leave as plain text
      if (match.index > last) fragment.append(doc.createTextNode(text.slice(last, match.index)))
      const chip = doc.createElement('span')
      chip.className = 'mention-chip mention-chip--msg'
      chip.textContent = `@${match[1]}`
      fragment.append(chip)
      last = match.index + match[0].length
    }
    if (last === 0) continue // nothing chipped
    if (last < text.length) fragment.append(doc.createTextNode(text.slice(last)))
    node.replaceWith(fragment)
  }
}

export function renderMarkdown(source: string, options: MarkdownOptions = {}): string {
  const rendered = marked.parse(source, { async: false, breaks: options.breaks ?? false }) as string
  const sanitized = DOMPurify.sanitize(rendered, { FORBID_TAGS, FORBID_ATTR })
  const doc = new DOMParser().parseFromString(sanitized, 'text/html')

  // Do not let rendered text silently contact remote image hosts. In a preview
  // that would leak repository reads; in chat it would turn any message into a
  // read receipt for whoever wrote it. Attachments have their own blob route.
  doc.querySelectorAll('img').forEach(img => {
    const placeholder = doc.createElement('figure')
    placeholder.className = 'markdown-preview__image'
    const label = doc.createElement('strong')
    label.textContent = img.alt || 'IMAGE'
    const sourceLabel = doc.createElement('figcaption')
    sourceLabel.textContent = img.getAttribute('src') || ''
    placeholder.append(label, sourceLabel)
    img.replaceWith(placeholder)
  })
  doc.querySelectorAll('a').forEach(link => {
    link.setAttribute('target', '_blank')
    link.setAttribute('rel', 'noreferrer noopener')
  })

  // After sanitizing, so the chips we add are not themselves scrubbed.
  if (options.mentions?.length) chipMentions(doc, options.mentions)

  return doc.body.innerHTML
}

/** A chat message body: newlines are literal, and mentions are chipped. */
export function renderChatMarkdown(text: string, mentions: string[] = []): string {
  return renderMarkdown(text, { breaks: true, mentions })
}
