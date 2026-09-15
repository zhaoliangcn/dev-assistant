<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { marked } from 'marked'
import hljs from 'highlight.js'

const props = withDefaults(defineProps<{
  content: string
  breaks?: boolean
}>(), {
  breaks: true,
})

marked.setOptions({
  breaks: props.breaks,
  gfm: true,
})

const renderer = new marked.Renderer()
renderer.code = function ({ text, lang }: { text: string; lang?: string }) {
  const validLang = lang && hljs.getLanguage(lang) ? lang : 'plaintext'
  let highlighted: string
  try {
    highlighted = hljs.highlight(text, { language: validLang }).value
  } catch {
    highlighted = hljs.highlightAuto(text).value
  }
  const langLabel = lang || 'text'
  return `<div class="vue-code-block">
    <div class="vue-code-header">
      <span class="vue-code-lang">${langLabel}</span>
      <button class="vue-code-copy" data-code="${encodeURIComponent(text)}">复制</button>
    </div>
    <pre><code class="hljs language-${validLang}">${highlighted}</code></pre>
  </div>`
}

marked.use({ renderer })

const html = computed(() => {
  if (!props.content) return ''
  try {
    return marked.parse(props.content) as string
  } catch {
    return props.content
  }
})

const containerRef = ref<HTMLElement>()

function handleCopyClick(e: Event) {
  const btn = e.target as HTMLElement
  if (!btn.classList.contains('vue-code-copy')) return
  const encoded = btn.getAttribute('data-code')
  if (!encoded) return
  navigator.clipboard.writeText(decodeURIComponent(encoded))
  btn.textContent = '已复制'
  setTimeout(() => { btn.textContent = '复制' }, 2000)
}

onMounted(() => {
  containerRef.value?.addEventListener('click', handleCopyClick)
})

onUnmounted(() => {
  containerRef.value?.removeEventListener('click', handleCopyClick)
})

watch(html, () => {
  // 内容更新后重新高亮
})
</script>

<template>
  <div ref="containerRef" class="markdown-body" v-html="html" />
</template>

<style>
.markdown-body {
  line-height: 1.6;
  word-wrap: break-word;
}

.markdown-body p {
  margin: 6px 0;
}

.markdown-body code {
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 0.9em;
  background: var(--color-code-bg);
  color: var(--color-code-text);
}

.markdown-body pre {
  margin: 8px 0;
  border-radius: 8px;
  overflow: hidden;
}

.markdown-body .vue-code-block {
  border-radius: 8px;
  overflow: hidden;
  background: #1e1e2e;
  margin: 8px 0;
}

.markdown-body .vue-code-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 12px;
  background: #181825;
  border-bottom: 1px solid #313244;
}

.markdown-body .vue-code-lang {
  font-size: 11px;
  color: #a6adc8;
  text-transform: uppercase;
  font-weight: 600;
}

.markdown-body .vue-code-copy {
  padding: 2px 8px;
  border-radius: 4px;
  border: none;
  background: #313244;
  color: #cdd6f4;
  font-size: 11px;
  cursor: pointer;
}

.markdown-body .vue-code-copy:hover {
  background: #45475a;
}

.markdown-body pre {
  margin: 0;
  padding: 12px;
}

.markdown-body pre code {
  background: transparent;
  padding: 0;
  color: #cdd6f4;
}

.markdown-body ul,
.markdown-body ol {
  padding-left: 20px;
  margin: 6px 0;
}

.markdown-body blockquote {
  margin: 8px 0;
  padding: 4px 12px;
  border-left: 3px solid var(--color-accent);
  color: var(--color-text-secondary);
}

.markdown-body table {
  border-collapse: collapse;
  width: 100%;
  margin: 8px 0;
}

.markdown-body th,
.markdown-body td {
  border: 1px solid var(--color-border);
  padding: 6px 10px;
  text-align: left;
  font-size: 13px;
}

.markdown-body th {
  background: var(--color-surface);
  font-weight: 600;
}

/* dark mode code */
.dark .markdown-body .vue-code-block {
  background: #1e1e2e;
}

.dark .markdown-body .vue-code-header {
  background: #11111b;
}
</style>