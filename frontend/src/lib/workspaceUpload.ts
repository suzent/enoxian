import { createFile, uploadWorkspaceFile, MAX_ATTACHMENT_BYTES } from '../api'

export const MAX_WORKSPACE_UPLOAD = 512 * 1024
export const MAX_WORKSPACE_MEDIA_UPLOAD = MAX_ATTACHMENT_BYTES ?? 10 * 1024 * 1024

const BINARY_MEDIA_EXTENSIONS = new Set([
  'png', 'jpg', 'jpeg', 'gif', 'webp', 'avif', 'bmp', 'ico',
  'mp3', 'wav', 'ogg', 'flac', 'm4a', 'aac', 'opus',
  'mp4', 'webm', 'mov', 'm4v', 'ogv',
  'pdf',
])

export function isBinaryMediaPath(path: string): boolean {
  const dot = path.lastIndexOf('.')
  if (dot < 0 || dot === path.length - 1) return false
  return BINARY_MEDIA_EXTENSIONS.has(path.slice(dot + 1).toLowerCase())
}

export async function importWorkspaceFiles(circleId: string, files: File[]) {
  const imported: string[] = [], errors: string[] = []
  for (const file of files) {
    try {
      const bytes = await file.arrayBuffer()
      if (isBinaryMediaPath(file.name)) {
        if (file.size > MAX_WORKSPACE_MEDIA_UPLOAD) {
          throw new Error('Media files must be 10 MB or smaller.')
        }
        await uploadWorkspaceFile(circleId, file.name, bytes)
        imported.push(file.name)
        continue
      }
      if (file.size > MAX_WORKSPACE_UPLOAD) throw new Error('Text files must be 512 KB or smaller.')
      let content: string
      try { content = new TextDecoder('utf-8', { fatal: true }).decode(bytes) }
      catch { throw new Error('Use chat attachments for binary files, such as PDFs and images.') }
      if (content.includes('\0')) throw new Error('Use chat attachments for binary files.')
      if (content.startsWith('%PDF-') || (file.type?.startsWith('image/') && file.type !== 'image/svg+xml')) {
        await uploadWorkspaceFile(circleId, file.name, bytes)
        imported.push(file.name)
        continue
      }
      await createFile(circleId, file.name, content)
      imported.push(file.name)
    } catch (error) { errors.push(`${file.name}: ${error instanceof Error ? error.message : 'Upload failed'}`) }
  }
  return { imported, errors }
}
