import { createFile } from '../api'

export const MAX_WORKSPACE_UPLOAD = 512 * 1024
export async function importWorkspaceFiles(circleId: string, files: File[]) {
  const imported: string[] = [], errors: string[] = []
  for (const file of files) {
    try {
      if (file.size > MAX_WORKSPACE_UPLOAD) throw new Error('Text files must be 512 KB or smaller.')
      const bytes = await file.arrayBuffer()
      let content: string
      try { content = new TextDecoder('utf-8', { fatal: true }).decode(bytes) }
      catch { throw new Error('Use chat attachments for binary files, such as PDFs and images.') }
      if (content.includes('\0')) throw new Error('Use chat attachments for binary files.')
      if (content.startsWith('%PDF-') || (file.type?.startsWith('image/') && file.type !== 'image/svg+xml')) {
        throw new Error('Use chat attachments for PDFs and images.')
      }
      await createFile(circleId, file.name, content)
      imported.push(file.name)
    } catch (error) { errors.push(`${file.name}: ${error instanceof Error ? error.message : 'Upload failed'}`) }
  }
  return { imported, errors }
}
