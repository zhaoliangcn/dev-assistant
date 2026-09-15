import { marked } from 'marked'
import hljs from 'highlight.js'
import { computed, ref } from 'vue'

// 配置 marked
marked.setOptions({
  breaks: true,
  gfm: true,
})

// 自定义渲染器
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
  return `<div class="code-block" data-lang="${langLabel}">
    <div class="code-block-header">
      <span class="code-lang">${langLabel}</span>
      <button class="code-copy-btn" onclick="(function(btn){var p=btn.closest('.code-block');var c=p.querySelector('code').textContent;navigator.clipboard.writeText(c);btn.textContent='已复制';setTimeout(function(){btn.textContent='复制'},2000)})(this)">复制</button>
    </div>
    <pre><code class="hljs language-${validLang}">${highlighted}</code></pre>
  </div>`
}

marked.use({ renderer })

export function useMarkdown() {
  const rawText = ref('')

  const html = computed(() => {
    if (!rawText.value) return ''
    try {
      return marked.parse(rawText.value) as string
    } catch {
      return rawText.value
    }
  })

  return { rawText, html }
}