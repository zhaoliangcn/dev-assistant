<script setup lang="ts">
defineProps<{
  connected: boolean
}>()
</script>

<template>
  <span :class="['status-badge', connected ? 'online' : 'offline']">
    <span class="dot" />
    {{ connected ? '已连接' : '离线' }}
  </span>
</template>

<style scoped>
.status-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  padding: 2px 8px;
  border-radius: 10px;
}

.status-badge.online {
  color: var(--color-success);
  background: color-mix(in srgb, var(--color-success) 12%, transparent);
}

.status-badge.offline {
  color: var(--color-text-muted);
  background: var(--color-hover);
}

.dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
}

.status-badge.online .dot {
  animation: pulse 2s infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1 }
  50% { opacity: 0.4 }
}
</style>