<script setup lang="ts">
import { watch, ref, onUnmounted } from 'vue'
import { useChatStore, useSessionsStore } from '@/stores'
import { getSession, toLocalMessages } from '@/api/sessions'
import ChatMessageList from '@/components/chat/ChatMessageList.vue'
import ChatInput from '@/components/chat/ChatInput.vue'
import ToolActivity from '@/components/chat/ToolActivity.vue'
import ReasoningPanel from '@/components/chat/ReasoningPanel.vue'
import ProjectPicker from '@/components/chat/ProjectPicker.vue'

const chat = useChatStore()
const sessions = useSessionsStore()
const showTools = ref(true)

chat.connectWs()

onUnmounted(() => {
  chat.disconnectWs()
})

function handleSend(content: string) {
  chat.sendMessage(content)
}

function handleStop() {
  chat.stopGeneration()
}

watch(() => sessions.activeId, async (id) => {
  if (id) {
    try {
      const detail = await getSession(id)
      chat.setSessionId(id)
      chat.loadMessages(toLocalMessages(detail))
    } catch (err) {
      console.error('加载会话失败:', err)
    }
  }
})

watch(() => chat.sessionId, (id) => {
  if (id) {
    sessions.setActive(id)
  }
})
</script>

<template>
  <div class="chat-view">
    <!-- 💭 思考流面板：始终渲染（不限消息数量），由自身 v-if 控制显隐 -->
    <ReasoningPanel
      :text="chat.reasoningText"
      :active="chat.reasoningActive"
      :visible="true"
    />

    <!-- 消息区域 -->
    <div class="chat-main">
      <!-- 欢迎屏：无消息时显示项目目录选择器 -->
      <template v-if="chat.messages.length === 0">
        <ProjectPicker />
      </template>

      <template v-else>
        <ChatMessageList :messages="chat.messages" />

        <ToolActivity
          :tool-messages="chat.toolMessages"
          :visible="showTools && chat.toolMessages.length > 0"
        />
      </template>
    </div>

    <!-- 输入框始终可见 -->
    <ChatInput
      :busy="chat.busy"
      @send="handleSend"
      @stop="handleStop"
    />
  </div>
</template>

<style scoped>
.chat-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
}

.chat-main {
  flex: 1;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}
</style>