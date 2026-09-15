<script setup lang="ts">
import type { ToolMessage } from '@/types'

defineProps<{
  toolMessages: ToolMessage[]
  visible?: boolean
}>()

defineEmits<{
  toggle: []
}>()

function formatArgs(args: Record<string, unknown>) {
  try {
    return JSON.stringify(args, null, 2)
  } catch {
    return String(args)
  }
}

function statusIcon(status: ToolMessage['status']) {
  if (status === 'running') return '⚙'
  if (status === 'done') return '✓'
  return '✗'
}
</script>

<template>
  <div v-if="visible && toolMessages.length > 0" class="tool-activity">
    <div class="tool-header">
      <span class="tool-title">工具活动</span>
    </div>
    <div class="tool-items">
      <div v-for="tm in toolMessages" :key="tm.id" :class="['tool-item', `tool-item--${tm.status}`]">
        <div class="tool-item-header">
          <span class="tool-item-icon">{{ statusIcon(tm.status) }}</span>
          <span class="tool-item-name">{{ tm.toolName }}</span>
        </div>
        <pre class="tool-args">{{ formatArgs(tm.args) }}</pre>
        <div v-if="tm.result" class="tool-result">{{ tm.result.slice(0, 500) }}{{ tm.result.length > 500 ? '...' : '' }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.tool-activity {
  border-top: 1px solid var(--color-border);
  background: var(--color-surface);
  max-height: 260px;
  overflow-y: auto;
}

.tool-header {
  padding: 8px 16px;
  border-bottom: 1px solid var(--color-border);
  position: sticky;
  top: 0;
  background: var(--color-surface);
  z-index: 1;
}

.tool-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text-secondary);
}

.tool-items {
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.tool-item {
  padding: 8px;
  border-radius: 8px;
  font-size: 12px;
}

.tool-item--running {
  background: color-mix(in srgb, var(--color-accent) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-accent) 20%, transparent);
}

.tool-item--done {
  background: color-mix(in srgb, var(--color-success) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-success) 20%, transparent);
}

.tool-item--error {
  background: color-mix(in srgb, var(--color-danger) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-danger) 20%, transparent);
}

.tool-item-header {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 4px;
}

.tool-item-name {
  font-weight: 600;
  color: var(--color-text-primary);
}

.tool-args {
  margin: 4px 0;
  padding: 6px;
  border-radius: 4px;
  background: var(--color-code-bg);
  font-size: 11px;
  overflow-x: auto;
  max-height: 80px;
  color: var(--color-text-secondary);
}

.tool-result {
  margin-top: 4px;
  font-size: 11px;
  color: var(--color-text-muted);
  white-space: pre-wrap;
  max-height: 80px;
  overflow-y: auto;
}
</style>