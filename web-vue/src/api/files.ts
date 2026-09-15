import { get, post } from './client'
import type { ListFilesResponse, FileContent } from '@/types'

export async function listDir(dirPath?: string): Promise<ListFilesResponse> {
  const query = dirPath ? { path: dirPath } : undefined
  return get<ListFilesResponse>('/files', query as Record<string, string | number> | undefined)
}

export async function getFileContent(
  filePath: string,
  offset?: number,
  limit?: number,
): Promise<FileContent> {
  const query: Record<string, string | number> = { path: filePath }
  if (offset !== undefined) query.offset = offset
  if (limit !== undefined) query.limit = limit
  return get<FileContent>('/files/content', query)
}

export async function saveFile(filePath: string, content: string): Promise<void> {
  await post('/files/save', { path: filePath, content })
}
