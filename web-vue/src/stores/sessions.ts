import { defineStore } from 'pinia'
import { ref } from 'vue'
import type { SessionSummary } from '@/types'
import { listSessions, deleteSession, renameSession } from '@/api/sessions'

export const useSessionsStore = defineStore('sessions', () => {
  const sessions = ref<SessionSummary[]>([])
  const activeId = ref<string | null>(null)
  const loading = ref(false)

  async function load() {
    loading.value = true
    try {
      sessions.value = await listSessions()
    } catch (err) {
      console.error('加载会话列表失败:', err)
    } finally {
      loading.value = false
    }
  }

  async function remove(id: string) {
    await deleteSession(id)
    sessions.value = sessions.value.filter(s => s.id !== id)
    if (activeId.value === id) {
      activeId.value = null
    }
  }

  async function rename(id: string, title: string) {
    await renameSession(id, title)
    const s = sessions.value.find(s => s.id === id)
    if (s) s.title = title
  }

  function setActive(id: string | null) {
    activeId.value = id
  }

  return { sessions, activeId, loading, load, remove, rename, setActive }
})