import { describe, expect, it, vi } from 'vitest'
import { importWorkspaceFiles } from '../workspaceUpload'
import { createFile, uploadWorkspaceFile } from '../../api'
vi.mock('../../api', () => ({ createFile: vi.fn(), uploadWorkspaceFile: vi.fn(), MAX_ATTACHMENT_BYTES: 10 * 1024 * 1024 }))
function file(name: string, bytes: Uint8Array) {
  return { name, size: bytes.length, arrayBuffer: async () => bytes.buffer } as File
}
describe('workspace imports', () => {
  it('preserves UTF-8 and continues past conflicts without overwriting them', async () => {
    vi.mocked(createFile).mockRejectedValueOnce(new Error('file already exists')).mockResolvedValueOnce({ status: 'created', path: 'notes.md' })
    const result = await importWorkspaceFiles('circle', [file('existing.md', new TextEncoder().encode('keep')), file('notes.md', new TextEncoder().encode('你好'))])
    expect(result.imported).toEqual(['notes.md'])
    expect(result.errors).toEqual(['existing.md: file already exists'])
    expect(createFile).toHaveBeenLastCalledWith('circle', 'notes.md', '你好')
  })
  it('uploads multimedia files via uploadWorkspaceFile', async () => {
    vi.mocked(uploadWorkspaceFile).mockResolvedValueOnce({ status: 'created', path: 'photo.png' })
    const result = await importWorkspaceFiles('circle', [file('photo.png', new Uint8Array([0x89, 0x50, 0x4e, 0x47]))])
    expect(result.imported).toEqual(['photo.png'])
    expect(result.errors).toEqual([])
    expect(uploadWorkspaceFile).toHaveBeenCalledOnce()
  })
  it('rejects binary bytes before making a create request', async () => {
    vi.mocked(createFile).mockClear()
    const result = await importWorkspaceFiles('circle', [file('image.bin', new Uint8Array([255, 0, 128]))])
    expect(result.errors[0]).toContain('chat attachments')
    expect(createFile).not.toHaveBeenCalled()
  })
})
