<script setup lang="ts">
import { ref, computed, nextTick } from 'vue'

const props = defineProps<{
  busy: boolean
  disabled?: boolean
}>()

const emit = defineEmits<{
  send: [content: string]
  stop: []
}>()

const inputText = ref('')
const textareaRef = ref<HTMLTextAreaElement>()

const canSend = computed(() => inputText.value.trim() && !props.busy)

function autoResize() {
  const el = textareaRef.value
  if (!el) return
  el.style.height = 'auto'
  el.style.height = Math.min(el.scrollHeight, 200) + 'px'
}

function handleKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    send()
  }
}

function send() {
  const text = inputText.value.trim()
  if (!text || props.busy) return
  emit('send', text)
  inputText.value = ''
  nextTick(() => autoResize())
}

function handleStop() {
  emit('stop')
}
</script>

<template>
  <div class="chat-input">
    <div class="input-wrapper">
      <textarea
        ref="textareaRef"
        v-model="inputText"
        :disabled="disabled"
        class="input-textarea"
        placeholder="输入消息... (Shift+Enter 换行，Enter 发送)"
        rows="1"
        @input="autoResize"
        @keydown="handleKeydown"
      />
      <div class="input-actions">
        <button
          v-if="busy"
          class="stop-btn"
          @click="handleStop"
          title="停止生成"
        >
          ■
        </button>
        <button
          v-else
          :disabled="!canSend"
          class="send-btn"
          @click="send"
          title="发送"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
            <path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z" />
          </svg>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.chat-input {
  padding: 12px 16px;
  border-top: 1px solid var(--color-border);
  background: var(--color-surface);
}

.input-wrapper {
  display: flex;
  align-items: flex-end;
  gap: 8px;
  background: var(--color-input);
  border: 1px solid var(--color-border);
  border-radius: 12px;
  padding: 8px 8px 8px 14px;
}

.input-wrapper:focus-within {
  border-color: var(--color-accent);
}

.input-textarea {
  flex: 1;
  border: none;
  background: transparent;
  color: var(--color-text-primary);
  font-size: 14px;
  font-family: inherit;
  resize: none;
  outline: none;
  max-height: 200px;
  line-height: 1.5;
}

.input-textarea::placeholder {
  color: var(--color-text-muted);
}

.input-actions {
  display: flex;
  align-items: center;
  gap: 4px;
}

.send-btn,
.stop-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 36px;
  height: 36px;
  border-radius: 8px;
  border: none;
  cursor: pointer;
  transition: background 0.15s;
}

.send-btn {
  background: var(--color-accent);
  color: #fff;
}

.send-btn:hover:not(:disabled) {
  opacity: 0.9;
}

.send-btn:disabled {
  background: var(--color-hover);
  color: var(--color-text-muted);
  cursor: not-allowed;
}

.stop-btn {
  background: var(--color-danger);
  color: #fff;
  font-size: 16px;
}

.stop-btn:hover {
  opacity: 0.9;
}
</style>