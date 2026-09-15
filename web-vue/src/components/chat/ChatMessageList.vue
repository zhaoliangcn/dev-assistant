<script setup lang="ts">
import { ref, watch, nextTick } from 'vue'
import type { ChatMessage as ChatMessageType } from '@/types'
import ChatMessage from './ChatMessage.vue'

const props = defineProps<{
  messages: ChatMessageType[]
}>()

const containerRef = ref<HTMLElement>()

watch(
  () => props.messages.length,
  () => {
    nextTick(() => scrollToBottom())
  }
)

watch(
  () => props.messages[props.messages.length - 1]?.content,
  () => {
    nextTick(() => scrollToBottom())
  }
)

function scrollToBottom() {
  const el = containerRef.value
  if (el) {
    el.scrollTop = el.scrollHeight
  }
}
</script>

<template>
  <div ref="containerRef" class="message-list">
    <div v-if="messages.length === 0" class="empty-state">
      <div class="empty-icon">💬</div>
      <p class="empty-text">发送消息开始对话</p>
    </div>
    <ChatMessage
      v-for="msg in messages"
      :key="msg.id"
      :message="msg"
    />
  </div>
</template>

<style scoped>
.message-list {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
  display: flex;
  flex-direction: column;
  scroll-behavior: smooth;
}

.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  flex: 1;
  gap: 12px;
}

.empty-icon {
  font-size: 48px;
  opacity: 0.3;
}

.empty-text {
  color: var(--color-text-muted);
  font-size: 14px;
}
</style>