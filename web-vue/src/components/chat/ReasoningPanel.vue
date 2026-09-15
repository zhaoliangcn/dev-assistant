<script setup lang="ts">
defineProps<{
  text: string
  active: boolean
  visible?: boolean
}>()
</script>

<template>
  <div v-if="visible && (text || active)" class="reasoning-panel">
    <div class="reasoning-header">
      <span class="reasoning-title">🤔 思考中...</span>
      <span v-if="active" class="reasoning-dot" />
    </div>
    <div class="reasoning-content">
      {{ text || '(开始思考...)' }}
      <span v-if="active" class="cursor">|</span>
    </div>
  </div>
</template>

<style scoped>
.reasoning-panel {
  padding: 10px 16px;
  background: color-mix(in srgb, var(--color-accent) 5%, transparent);
  border-bottom: 1px solid color-mix(in srgb, var(--color-accent) 15%, transparent);
}

.reasoning-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 4px;
}

.reasoning-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--color-text-secondary);
}

.reasoning-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-accent);
  animation: pulse 1.5s infinite;
}

.reasoning-content {
  font-size: 13px;
  color: var(--color-text-secondary);
  white-space: pre-wrap;
  line-height: 1.5;
}

.cursor {
  animation: blink 0.8s infinite;
  color: var(--color-accent);
}

@keyframes pulse {
  0%, 100% { opacity: 1 }
  50% { opacity: 0.4 }
}

@keyframes blink {
  0%, 100% { opacity: 1 }
  50% { opacity: 0 }
}
</style>