import { describe, it, expect } from 'vitest'
import { renderChatMarkdown, renderMarkdown } from '../markdown'

/**
 * Chat is where people paste commands and Windows paths. Markdown would read
 * `\.` and `\*` as escapes and drop the backslash, quietly turning
 * `C:\Users\.enoxian\*.log` into a different path.
 */
describe('chat rendering keeps backslashes', () => {
  it('shows a Windows path exactly as written', () => {
    const html = renderChatMarkdown('$env:USERPROFILE\\.enoxian\\logs\\*.log')
    expect(html).toContain('$env:USERPROFILE\\.enoxian\\logs\\*.log')
  })

  it('still escapes HTML after a backslash', () => {
    const html = renderChatMarkdown('\\<script>alert(1)</script>')
    expect(html).not.toContain('<script')
    expect(html).toContain('\\&lt;script&gt;')
  })

  it('still formats markdown and keeps code literal', () => {
    const html = renderChatMarkdown('**bold** and `a\\.b`')
    expect(html).toContain('<strong>bold</strong>')
    expect(html).toContain('<code>a\\.b</code>')
  })

  it('leaves file previews as standard markdown', () => {
    expect(renderMarkdown('a\\.b')).toContain('a.b')
  })
})
