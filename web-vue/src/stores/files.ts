import { defineStore } from 'pinia'
import { ref } from 'vue'
import type { FileEntry, FileContent } from '@/types'
import { listDir, getFileContent, saveFile } from '@/api/files'

export const useFilesStore = defineStore('files', () => {
  const currentPath = ref('')
  const entries = ref<FileEntry[]>([])
  const fileContent = ref<FileContent | null>(null)
  const loading = ref(false)

  async function browse(dirPath?: string) {
    loading.value = true
    try {
      const res = await listDir(dirPath)
      entries.value = res.entries
      currentPath.value = res.current_path || ''
    } finally {
      loading.value = false
    }
  }

  async function openFile(filePath: string, offset?: number, limit?: number) {
    loading.value = true
    try {
      fileContent.value = await getFileContent(filePath, offset, limit)
    } finally {
      loading.value = false
    }
  }

  async function save(filePath: string, content: string) {
    await saveFile(filePath, content)
    fileContent.value = {
      path: filePath,
      content,
      total_lines: content.split('\n').length,
    }
  }

  function goUp() {
    if (!currentPath.value) return
    const sep = currentPath.value.includes('\\') ? '\\' : '/'
    const parent = currentPath.value.split(sep).slice(0, -1).join(sep) || ''
    browse(parent)
  }

  return { currentPath, entries, fileContent, loading, browse, openFile, save, goUp }
})
