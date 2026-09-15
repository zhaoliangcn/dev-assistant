<script setup lang="ts">
import { useFilesStore } from '@/stores'

const files = useFilesStore()
</script>

<template>
  <div class="files-view">
    <div class="files-header">
      <h2>文件浏览</h2>
      <span class="files-path">{{ files.currentPath || '/' }}</span>
    </div>

    <div v-if="files.loading" class="loading">加载中...</div>

    <div class="files-list" v-if="!files.loading">
      <div
        v-for="entry in files.entries"
        :key="entry.path"
        class="file-entry"
        @click="entry.is_dir ? files.browse(entry.path) : files.openFile(entry.path)"
      >
        <span class="file-icon">{{ entry.is_dir ? '📁' : '📄' }}</span>
        <span class="file-name">{{ entry.name }}</span>
        <span class="file-size">{{ entry.size ? `${(entry.size / 1024).toFixed(1)} KB` : '' }}</span>
      </div>
    </div>

    <div v-if="files.fileContent" class="file-content-view">
      <div class="file-content-header">
        <span>{{ files.currentPath }}</span>
        <span class="line-count">{{ files.fileContent.total_lines }} 行</span>
      </div>
      <pre class="file-content">{{ files.fileContent.content }}</pre>
    </div>
  </div>
</template>

<style scoped>
.files-view {
  padding: 16px;
  height: 100%;
  overflow-y: auto;
}

.files-header {
  margin-bottom: 16px;
}

.files-header h2 {
  font-size: 18px;
  margin: 0 0 4px;
  color: var(--color-text-primary);
}

.files-path {
  font-size: 12px;
  color: var(--color-text-muted);
  font-family: monospace;
}

.loading {
  text-align: center;
  padding: 32px;
  color: var(--color-text-muted);
}

.files-list {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
  gap: 6px;
}

.file-entry {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.15s;
}

.file-entry:hover {
  background: var(--color-hover);
}

.file-icon {
  font-size: 18px;
}

.file-name {
  font-size: 13px;
  color: var(--color-text-primary);
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-size {
  font-size: 11px;
  color: var(--color-text-muted);
}

.file-content-view {
  margin-top: 16px;
  border: 1px solid var(--color-border);
  border-radius: 8px;
  overflow: hidden;
}

.file-content-header {
  display: flex;
  justify-content: space-between;
  padding: 8px 12px;
  background: var(--color-surface);
  border-bottom: 1px solid var(--color-border);
  font-size: 13px;
  color: var(--color-text-secondary);
}

.file-content {
  margin: 0;
  padding: 12px;
  font-size: 13px;
  max-height: 600px;
  overflow: auto;
  background: var(--color-code-bg);
  color: var(--color-text-primary);
}
</style>