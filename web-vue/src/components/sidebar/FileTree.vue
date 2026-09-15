<script setup lang="ts">
import { useFilesStore } from '@/stores'

const files = useFilesStore()

function enterDir(entry: { is_dir: boolean; path: string }) {
  if (entry.is_dir) {
    files.browse(entry.path)
  } else {
    files.openFile(entry.path)
  }
}

function formatSize(bytes?: number) {
  if (!bytes) return ''
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}
</script>

<template>
  <div class="file-tree">
    <div class="tree-nav" v-if="files.currentPath">
      <button class="nav-btn" @click="files.goUp" title="上级目录">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M19 12H5M12 19l-7-7 7-7" />
        </svg>
      </button>
      <span class="current-path">{{ files.currentPath || '/' }}</span>
    </div>

    <div v-if="files.loading" class="loading">加载中...</div>

    <ul class="tree-items" v-if="!files.loading">
      <li
        v-for="entry in files.entries"
        :key="entry.path"
        class="tree-item"
        @click="enterDir(entry)"
      >
        <svg v-if="entry.is_dir" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="tree-icon">
          <path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z" />
        </svg>
        <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="tree-icon">
          <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z" />
          <polyline points="14 2 14 8 20 8" />
        </svg>
        <span class="tree-name">{{ entry.name }}</span>
        <span class="tree-size" v-if="entry.size">{{ formatSize(entry.size) }}</span>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.file-tree {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.tree-nav {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px;
}

.nav-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 4px;
  border: none;
  background: transparent;
  color: var(--color-text-secondary);
  cursor: pointer;
}

.nav-btn:hover {
  background: var(--color-hover);
}

.current-path {
  font-size: 12px;
  color: var(--color-text-muted);
  font-family: monospace;
}

.loading {
  text-align: center;
  color: var(--color-text-muted);
  font-size: 13px;
  padding: 16px 0;
}

.tree-items {
  list-style: none;
  padding: 0;
  margin: 0;
}

.tree-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 8px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 13px;
  color: var(--color-text-primary);
  transition: background 0.15s;
}

.tree-item:hover {
  background: var(--color-hover);
}

.tree-icon {
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.tree-name {
  flex: 1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tree-size {
  font-size: 11px;
  color: var(--color-text-muted);
  flex-shrink: 0;
}
</style>