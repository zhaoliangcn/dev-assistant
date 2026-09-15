<script setup lang="ts">
import { ref } from 'vue'

const props = defineProps<{
  code: string
  lang?: string
}>()

const copied = ref(false)

async function copy() {
  await navigator.clipboard.writeText(props.code)
  copied.value = true
  setTimeout(() => { copied.value = false }, 2000)
}
</script>

<template>
  <div class="code-block">
    <div class="code-block-header">
      <span class="code-lang">{{ lang || 'text' }}</span>
      <button class="code-copy-btn" @click="copy">
        {{ copied ? '已复制' : '复制' }}
      </button>
    </div>
    <pre class="code-pre"><code :class="lang ? `language-${lang}` : ''">{{ code }}</code></pre>
  </div>
</template>

<style scoped>
.code-block {
  border-radius: 8px;
  overflow: hidden;
  margin: 8px 0;
  background: var(--color-code-bg);
  border: 1px solid var(--color-border);
}

.code-block-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 12px;
  background: var(--color-surface);
  border-bottom: 1px solid var(--color-border);
}

.code-lang {
  font-size: 11px;
  color: var(--color-text-muted);
  text-transform: uppercase;
  font-weight: 600;
}

.code-copy-btn {
  padding: 2px 8px;
  border-radius: 4px;
  border: none;
  background: var(--color-hover);
  color: var(--color-text-secondary);
  font-size: 11px;
  cursor: pointer;
}

.code-copy-btn:hover {
  background: var(--color-active);
  color: var(--color-text-primary);
}

.code-pre {
  margin: 0;
  padding: 12px;
  overflow-x: auto;
  font-size: 13px;
}

.code-pre code {
  background: transparent;
  padding: 0;
  color: var(--color-text-primary);
}
</style>