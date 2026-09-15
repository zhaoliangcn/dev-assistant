<script setup lang="ts">
import { useSessionsStore, useChatStore } from '@/stores'

const sessions = useSessionsStore()
const chat = useChatStore()

function selectSession(id: string) {
  sessions.setActive(id)
  // 触发加载历史会话（由 ChatView 监听 activeId 变化）
}

function newChat() {
  sessions.setActive(null)
  chat.newChat()
}

function formatDate(dateStr: string) {
  try {
    const d = new Date(dateStr)
    const now = new Date()
    const diff = now.getTime() - d.getTime()
    if (diff < 3600000) return `${Math.floor(diff / 60000)} 分钟前`
    if (diff < 86400000) return `${Math.floor(diff / 3600000)} 小时前`
    return d.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' })
  } catch {
    return dateStr
  }
}
</script>

<template>
  <div class="session-list">
    <button class="new-chat-btn" @click="newChat">
      + 新建对话
    </button>

    <div v-if="sessions.loading" class="loading">加载中...</div>

    <div v-if="!sessions.loading && sessions.sessions.length === 0" class="empty">
      暂无会话
    </div>

    <ul class="session-items" v-if="sessions.sessions.length > 0">
      <li
        v-for="s in sessions.sessions"
        :key="s.id"
        :class="{ active: sessions.activeId === s.id }"
        class="session-item"
        @click="selectSession(s.id)"
      >
        <div class="session-info">
          <span class="session-title">{{ s.title || '新对话' }}</span>
          <span class="session-meta">
            {{ s.message_count }} 条消息 · {{ formatDate(s.created_at) }}
          </span>
        </div>
        <div class="session-actions">
          <button class="action-btn" title="删除" @click.stop="sessions.remove(s.id)">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M3 6h18M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2" />
            </svg>
          </button>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.session-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.new-chat-btn {
  width: 100%;
  padding: 8px;
  border: 1px dashed var(--color-border);
  border-radius: 8px;
  background: transparent;
  color: var(--color-accent);
  font-size: 13px;
  cursor: pointer;
  transition: background 0.15s;
}

.new-chat-btn:hover {
  background: var(--color-hover);
}

.loading,
.empty {
  text-align: center;
  color: var(--color-text-muted);
  font-size: 13px;
  padding: 16px 0;
}

.session-items {
  list-style: none;
  padding: 0;
  margin: 0;
}

.session-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 10px;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.15s;
  gap: 8px;
}

.session-item:hover {
  background: var(--color-hover);
}

.session-item.active {
  background: var(--color-active);
}

.session-info {
  display: flex;
  flex-direction: column;
  min-width: 0;
  flex: 1;
}

.session-title {
  font-size: 13px;
  color: var(--color-text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.session-meta {
  font-size: 11px;
  color: var(--color-text-muted);
}

.session-actions {
  opacity: 0;
  transition: opacity 0.15s;
}

.session-item:hover .session-actions {
  opacity: 1;
}

.action-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 4px;
  border: none;
  background: transparent;
  color: var(--color-text-muted);
  cursor: pointer;
}

.action-btn:hover {
  background: var(--color-hover);
  color: var(--color-danger);
}
</style>