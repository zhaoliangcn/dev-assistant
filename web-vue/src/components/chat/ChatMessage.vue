<script setup lang="ts">
import type { ChatMessage as ChatMessageType } from '@/types'
import MarkdownRenderer from '@/components/shared/MarkdownRenderer.vue'
import TokenUsage from '@/components/shared/TokenUsage.vue'

const props = defineProps<{
  message: ChatMessageType
}>()

function formatTime(ts: number) {
  return new Date(ts).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
}
</script>

<template>
  <div :class="['message', `message--${message.role}`]">
    <div class="message-header">
      <span class="message-role">
        {{ message.role === 'user' ? '你' : message.role === 'assistant' ? '助手' : message.role === 'error' ? '错误' : '系统' }}
      </span>
      <span class="message-time">{{ formatTime(message.timestamp) }}</span>
      <span v-if="message.status === 'streaming'" class="streaming-indicator">●</span>
    </div>

    <div class="message-content">
      <div v-if="message.reasoning" class="reasoning-box">
        <details>
          <summary>思考过程</summary>
          <MarkdownRenderer :content="message.reasoning" />
        </details>
      </div>

      <template v-if="message.role === 'assistant' || message.role === 'error'">
        <MarkdownRenderer :content="message.content" />
      </template>
      <template v-else>
        <p class="plain-text">{{ message.content }}</p>
      </template>

      <div v-if="message.toolCalls && message.toolCalls.length > 0" class="tool-calls-inline">
        <div v-for="tc in message.toolCalls" :key="`${tc.toolName}_${message.id}`" class="tool-call-badge">
          <span class="tool-icon">{{ tc.pending ? '⚙' : tc.success !== false ? '✓' : '✗' }}</span>
          <span class="tool-name">{{ tc.toolName }}</span>
        </div>
      </div>

      <div v-if="message.status === 'cancelled'" class="cancelled-badge">已取消</div>

      <div v-if="message.tokenUsage" class="token-usage">
        <TokenUsage :usage="message.tokenUsage" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.message {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 10px 16px;
  max-width: 85%;
  border-radius: 12px;
  margin-bottom: 8px;
}

.message--user {
  align-self: flex-end;
  background: var(--color-accent);
  color: #fff;
}

.message--assistant {
  align-self: flex-start;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
}

.message--error {
  align-self: flex-start;
  background: color-mix(in srgb, var(--color-danger) 10%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-danger) 30%, transparent);
}

.message--system {
  align-self: center;
  background: var(--color-hover);
  font-size: 12px;
  color: var(--color-text-muted);
  max-width: 70%;
}

.message-header {
  display: flex;
  align-items: center;
  gap: 8px;
}

.message-role {
  font-size: 12px;
  font-weight: 600;
  opacity: 0.8;
}

.message-time {
  font-size: 11px;
  opacity: 0.5;
}

.streaming-indicator {
  color: var(--color-accent);
  animation: blink 0.8s infinite;
  font-size: 10px;
}

@keyframes blink {
  0%, 100% { opacity: 1 }
  50% { opacity: 0.3 }
}

.message-content {
  font-size: 14px;
}

.message--user .plain-text {
  color: #fff;
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
}

.reasoning-box {
  margin-bottom: 8px;
}

.reasoning-box details {
  font-size: 13px;
}

.reasoning-box summary {
  color: var(--color-text-secondary);
  cursor: pointer;
  font-size: 12px;
  margin-bottom: 4px;
}

.tool-calls-inline {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 8px;
}

.tool-call-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 6px;
  background: var(--color-hover);
  font-size: 11px;
  color: var(--color-text-secondary);
}

.tool-icon {
  font-size: 10px;
}

.cancelled-badge {
  margin-top: 6px;
  font-size: 12px;
  color: var(--color-text-muted);
  font-style: italic;
}

.token-usage {
  margin-top: 6px;
  font-size: 10px;
  color: var(--color-text-muted);
  opacity: 0.6;
}
</style>