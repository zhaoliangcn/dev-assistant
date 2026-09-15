<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useChatStore, useModelsStore, useAppStore } from '@/stores'
import { useI18n } from '@/composables/useI18n'
import StatusBadge from '@/components/shared/StatusBadge.vue'
import ModelSettingsPanel from '@/components/shared/ModelSettingsPanel.vue'

const chat = useChatStore()
const models = useModelsStore()
const app = useAppStore()
const { t } = useI18n()

const showSettings = ref(false)

onMounted(() => {
  models.loadModels()
})

function handleModelChange(e: Event) {
  const select = e.target as HTMLSelectElement
  models.switch(select.value)
}

function handleLocaleChange(e: Event) {
  const select = e.target as HTMLSelectElement
  app.setLocale(select.value as 'zh' | 'en')
}
</script>

<template>
  <div class="header">
    <div class="header-left">
      <button class="menu-btn" @click="app.toggleSidebar" :title="t('theme_toggle')">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M3 12h18M3 6h18M3 18h18" />
        </svg>
      </button>
      <span class="brand">Dev-Assistant</span>
      <StatusBadge :connected="chat.connected" />
      <span v-if="chat.projectDir || models.status?.project_dir" class="project-badge" :title="(chat.projectDir || models.status?.project_dir) as string">
        📁 {{ (chat.projectDir || models.status?.project_dir) as string }}
      </span>
    </div>

    <div class="header-center">
      <select
        v-if="models.models.length > 1"
        class="model-select"
        :value="models.activeModel"
        @change="handleModelChange"
        :title="t('model') + ': ' + models.activeModel"
      >
        <option v-for="m in models.models" :key="m.name" :value="m.name">
          {{ m.name }} ({{ m.provider }})
        </option>
      </select>
      <span v-else-if="models.activeModel" class="model-label">
        {{ t('model') }}: {{ models.activeModel }}
      </span>
    </div>

    <div class="header-right">
      <!-- 语言切换 -->
      <select
        class="lang-select"
        :value="app.locale"
        @change="handleLocaleChange"
        :title="t('language')"
      >
        <option value="zh">中文</option>
        <option value="en">English</option>
      </select>

      <!-- 模型设置 -->
      <button
        class="icon-btn"
        @click="showSettings = true"
        :title="t('model_settings')"
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83 0 2 2 0 010-2.83l.06-.06a1.65 1.65 0 00.33-1.82 1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 010-2.83 2 2 0 012.83 0l.06.06a1.65 1.65 0 001.82.33H9a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 0 2 2 0 010 2.83l-.06.06a1.65 1.65 0 00-.33 1.82V9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
        </svg>
      </button>

      <!-- 主题切换 -->
      <button
        class="icon-btn"
        @click="app.toggleTheme()"
        :title="app.isDark ? t('light_mode') : t('dark_mode')"
      >
        <svg v-if="app.isDark" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <circle cx="12" cy="12" r="5" />
          <path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42" />
        </svg>
        <svg v-else width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M21 12.79A9 9 0 1111.21 3 7 7 0 0021 12.79z" />
        </svg>
      </button>
    </div>
  </div>

  <ModelSettingsPanel v-if="showSettings" @close="showSettings = false" />
</template>

<style scoped>
.header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 100%;
  padding: 0 12px;
  gap: 12px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 8px;
}

.header-center {
  flex: 1;
  display: flex;
  justify-content: center;
}

.header-right {
  display: flex;
  align-items: center;
  gap: 8px;
}

.brand {
  font-weight: 600;
  font-size: 15px;
  color: var(--color-text-primary);
}

.project-badge {
  font-size: 11px;
  color: var(--color-text-muted);
  background: var(--color-hover);
  padding: 2px 8px;
  border-radius: 10px;
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: monospace;
}

.menu-btn {
  display: none;
}

.icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: 6px;
  border: none;
  background: transparent;
  color: var(--color-text-secondary);
  cursor: pointer;
}

.icon-btn:hover {
  background: var(--color-hover);
  color: var(--color-text-primary);
}

.model-select,
.lang-select {
  padding: 4px 8px;
  border-radius: 6px;
  border: 1px solid var(--color-border);
  background: var(--color-input);
  color: var(--color-text-primary);
  font-size: 13px;
}

.model-select {
  max-width: 260px;
}

.model-label {
  font-size: 13px;
  color: var(--color-text-secondary);
}

@media (max-width: 768px) {
  .menu-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 6px;
    border: none;
    background: transparent;
    color: var(--color-text-secondary);
    cursor: pointer;
  }
  .menu-btn:hover {
    background: var(--color-hover);
  }
  .lang-select {
    font-size: 11px;
  }
}
</style>
