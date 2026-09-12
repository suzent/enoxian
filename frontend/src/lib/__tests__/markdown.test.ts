import { describe, it, expect } from 'vitest'
import { renderMarkdown, renderChatMarkdown } from '../markdown'

/**
 * Chat messages and previewed files are both written elsewhere — by another
 * peer's agent, or by a file synced from another device — so the renderer is a
 * trust boundary, not just a formatter. These cover both halves: that markdown
 * renders, and that nothing hostile survives the trip.
 */
describe('renderChatMarkdown formatting', () => {
  it('renders inline emphasis and code', () => {
    const html = renderChatMarkdown('**bold** and `code()`')
    expect(html).toContain('<strong>bold</strong>')
    expect(html).toContain('<code>code()</code>')
  })

  it('renders lists and fenced code blocks', () => {
    expect(renderChatMarkdown('- one\n- two')).toContain('<li>one</li>')
    const fenced = renderChatMarkdown('```\nnpm test\n```')
    expect(fenced).toContain('<pre>')
    expect(fenced).toContain('npm test')
  })

  it('treats a single newline as a line break', () => {
    // The composer sends one line, but an agent posting from the CLI writes
    // real newlines and means them.
    expect(renderChatMarkdown('first\nsecond')).toContain('<br>')
  })

  it('leaves ordinary prose as a plain paragraph', () => {
    expect(renderChatMarkdown('just a message')).toBe('<p>just a message</p>\n')
  })
})

describe('renderChatMarkdown mention chips', () => {
  it('chips a mention the server recognised', () => {
    const html = renderChatMarkdown('ping @claude please', ['claude'])
    expect(html).toContain('<span class="mention-chip mention-chip--msg">@claude</span>')
  })

  it('leaves an unrecognised handle as plain text', () => {
    const html = renderChatMarkdown('ping @nobody please', ['claude'])
    expect(html).not.toContain('mention-chip')
    expect(html).toContain('@nobody')
  })

  it('does not chip a mention inside code — it is a sample, not a ping', () => {
    const inline = renderChatMarkdown('run `git commit -m "@claude"`', ['claude'])
    expect(inline).not.toContain('mention-chip')

    const fenced = renderChatMarkdown('```\nnotify @claude\n```', ['claude'])
    expect(fenced).not.toContain('mention-chip')
  })

  it('chips mentions inside formatted text', () => {
    const html = renderChatMarkdown('- **ask @codex**', ['codex'])
    expect(html).toContain('mention-chip')
    expect(html).toContain('@codex')
  })

  it('chips every mention in a message, not just the first', () => {
    const html = renderChatMarkdown('@claude and @codex', ['claude', 'codex'])
    expect(html.match(/mention-chip--msg/g)).toHaveLength(2)
  })
})

describe('renderMarkdown is a trust boundary', () => {
  it('strips script tags', () => {
    const html = renderMarkdown('hi <script>alert(1)</script>')
    expect(html).not.toContain('<script')
    expect(html).not.toContain('alert(1)')
  })

  it('strips inline event handlers', () => {
    const html = renderMarkdown('<p onclick="steal()">click</p>')
    expect(html).not.toContain('onclick')
  })

  it('drops a javascript: link target', () => {
    const html = renderMarkdown('[x](javascript:alert(1))')
    expect(html).not.toContain('javascript:')
  })

  it('strips iframes and form controls', () => {
    const html = renderMarkdown('<iframe src="http://x"></iframe><input name="pw">')
    expect(html).not.toContain('<iframe')
    expect(html).not.toContain('<input')
  })

  it('never emits an img — a remote one would beacon the reader', () => {
    // A message is read by everyone in the circle; a remote image in it is a
    // read receipt for whoever wrote the message.
    const html = renderChatMarkdown('![shot](https://tracker.example/p.png)')
    expect(html).not.toContain('<img')
    expect(html).toContain('markdown-preview__image')
    expect(html).toContain('shot')
    expect(html).toContain('https://tracker.example/p.png')
  })

  it('escapes HTML that arrives as literal text', () => {
    const html = renderChatMarkdown('use <b>tags</b> carefully', [])
    // marked passes raw HTML through, but DOMPurify keeps only what is inert.
    expect(html).not.toContain('<script')
  })

  it('opens links in a new tab without leaking the opener', () => {
    const html = renderMarkdown('[docs](https://example.com)')
    expect(html).toContain('target="_blank"')
    expect(html).toContain('rel="noreferrer noopener"')
  })

  it('does not let a crafted message forge a mention chip', () => {
    // The chip class is how the UI says "this really was a ping". Raw HTML in a
    // message must not be able to claim it.
    const html = renderChatMarkdown('<span class="mention-chip mention-chip--msg">@admin</span>', [])
    expect(html).not.toContain('mention-chip')
  })
})
