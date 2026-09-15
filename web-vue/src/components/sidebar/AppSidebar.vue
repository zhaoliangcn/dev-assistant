<script setup lang="ts">
import { useAppStore, useSessionsStore, useFilesStore } from '@/stores'
import { useRouter } from 'vue-router'
import { onMounted, watch } from 'vue'
import SessionList from '@/components/sidebar/SessionList.vue'
import FileTree from '@/components/sidebar/FileTree.vue'

const router = useRouter()
const app = useAppStore()
const sessions = useSessionsStore()
const files = useFilesStore()

onMounted(() => {
  sessions.load()
})

watch(() => router.currentRoute.value.path, (path) => {
  if (path === '/files') {
    app.setSidebarTab('files')
    files.browse()
  }
})
</script>

<template>
  <aside class="sidebar">
    <nav class="sidebar-tabs">
      <button
        :class="{ active: app.sidebarTab === 'sessions' }"
        @click="app.setSidebarTab('sessions')"
      >
        会话
      </button>
      <button
        :class="{ active: app.sidebarTab === 'files' }"
        @click="app.setSidebarTab('files'); files.browse()"
      >
        文件
      </button>
    </nav>

    <div class="sidebar-content" v-show="app.sidebarTab === 'sessions'">
      <SessionList />
    </div>

    <div class="sidebar-content" v-show="app.sidebarTab === 'files'">
      <FileTree />
    </div>
  </aside>
</template>

<style scoped>
.sidebar {
  display: flex;
  flex-direction: column;
  height: 100%;
}

.sidebar-tabs {
  display: flex;
  border-bottom: 1px solid var(--color-border);
  padding: 0 8px;
  gap: 4px;
}

.sidebar-tabs button {
  flex: 1;
  padding: 8px 0;
  border: none;
  background: transparent;
  color: var(--color-text-secondary);
  font-size: 13px;
  cursor: pointer;
  border-bottom: 2px solid transparent;
  transition: color 0.15s, border-color 0.15s;
}

.sidebar-tabs button.active {
  color: var(--color-accent);
  border-bottom-color: var(--color-accent);
}

.sidebar-tabs button:hover {
  color: var(--color-text-primary);
}

.sidebar-content {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
}
</style>