<script setup lang="ts">
import { computed } from 'vue'
import type { TokenUsage } from '@/types'

const props = defineProps<{
  usage: TokenUsage
}>()

function formatNum(n: number) {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`
  return String(n)
}

const total = computed(() => formatNum(props.usage.total_tokens))
const prompt = computed(() => formatNum(props.usage.prompt_tokens))
const completion = computed(() => formatNum(props.usage.completion_tokens))
</script>

<template>
  <div class="token-usage">
    <span class="tu-item tu-total" title="总 Token 数">
      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M2 12a10 10 0 1020 0 10 10 0 00-20 0" />
        <path d="M12 6v6l4 2" />
      </svg>
      {{ total }}
    </span>
    <span class="tu-item tu-prompt" title="输入 Token">
      ↑{{ prompt }}
    </span>
    <span class="tu-item tu-completion" title="输出 Token">
      ↓{{ completion }}
    </span>
  </div>
</template>

<style scoped>
.token-usage {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 10px;
  color: var(--color-text-muted);
}

.tu-item {
  display: inline-flex;
  align-items: center;
  gap: 2px;
}

.tu-total {
  font-weight: 600;
  color: var(--color-text-secondary);
}

.tu-prompt {
  color: var(--color-text-muted);
}

.tu-completion {
  color: var(--color-text-muted);
}
</style>
