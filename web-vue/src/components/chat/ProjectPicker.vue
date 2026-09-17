<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useChatStore, useModelsStore } from '@/stores'
import { useI18n } from '@/composables/useI18n'
import { getStatus } from '@/api/models'
import ModelSettingsPanel from '@/components/shared/ModelSettingsPanel.vue'

const chat = useChatStore()
const models = useModelsStore()
const { t } = useI18n()

const inputDir = ref(chat.projectDir || '')
const workingDir = ref('')
const loading = ref(false)
const showSettings = ref(false)

onMounted(async () => {
  try {
    const status = await getStatus()
    workingDir.value = status.project_dir
    if (!chat.projectDir) {
      inputDir.value = status.project_dir
    }
  } catch {
    // 后端未就绪时跳过
  }
  // 零配置首启：加载模型列表，为空时展示配置向导入口
  try {
    await models.loadModels()
  } catch {
    // 忽略
  }
})

async function applyProjectDir() {
  const dir = inputDir.value.trim()
  if (!dir) return
  loading.value = true
  chat.switchProject(dir)
  // 实际结果由 chat store 的 2s 超时逻辑判定
  setTimeout(() => {
    loading.value = false
  }, 2500)
}

async function pickSubdir() {
  // 列出 working_dir 下的子目录供快速选择
  try {
    const { listDir } = await import('@/api/files')
    const res = await listDir(workingDir.value)
    const dirs = res.entries.filter(e => e.is_dir)
    // 简单弹窗选择
    const choice = prompt(
      '选择子目录（在输入框中手动输入完整路径）:\n\n' +
      dirs.map(d => `• ${d.path}`).join('\n'),
    )
    if (choice) inputDir.value = choice
  } catch {
    // 忽略
  }
}
</script>

<template>
  <div class="project-picker">
    <div class="project-picker-header">
      <div class="welcome-logo">🤖</div>
      <div class="welcome-text">
        <h2>Dev-Assistant</h2>
        <p class="welcome-subtitle">{{ t('welcome_select_project') }}</p>
      </div>
    </div>

    <div class="project-picker-body">
      <!-- 零配置首启向导：未配置任何模型时提示 -->
      <div v-if="models.models.length === 0" class="first-run-banner">
        <div class="banner-text">
          <strong>👋 首次使用，先配置一个模型</strong>
          <span>填写 API URL 和 Key 即可开始，保存后立即生效，无需重启。</span>
        </div>
        <button class="btn-primary" @click="showSettings = true">
          ⚙️ 立即配置
        </button>
      </div>

      <div class="current-project-row">
        <span class="label">{{ t('current_project') }}:</span>
        <code class="path-display">{{ chat.projectDir || workingDir || '—' }}</code>
      </div>

      <div class="input-row">
        <input
          v-model="inputDir"
          type="text"
          class="path-input"
          :placeholder="t('project_dir_placeholder')"
          @keydown.enter="applyProjectDir"
          :disabled="loading"
        />
      </div>

      <div class="actions-row">
        <button
          class="btn-primary"
          @click="applyProjectDir"
          :disabled="loading || !inputDir.trim()"
        >
          <span v-if="loading">⏳</span>
          <span v-else>✓</span>
          {{ t('switch_project') }}
        </button>
        <button
          class="btn-secondary"
          @click="pickSubdir"
          :disabled="loading"
          title="浏览子目录"
        >
          📂 浏览
        </button>
      </div>

      <p v-if="chat.projectError" class="error-msg">⚠️ {{ chat.projectError }}</p>

      <div v-if="workingDir" class="hint">
        💡 {{ t('working_dir_hint') }}: <code>{{ workingDir }}</code>
      </div>

      <div v-if="models.models.length > 0" class="current-model">
        <span class="label">{{ t('model') }}:</span>
        <strong>{{ models.activeModel }}</strong>
        <button class="link-btn" @click="showSettings = true">{{ t('model_settings') }}</button>
      </div>
    </div>

    <ModelSettingsPanel
      v-if="showSettings"
      @close="showSettings = false"
    />
  </div>
</template>

<style scoped>
.project-picker {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 40px 20px;
  gap: 24px;
}

.project-picker-header {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}

.welcome-logo {
  font-size: 64px;
  line-height: 1;
}

.welcome-text {
  text-align: center;
}

.welcome-text h2 {
  margin: 0;
  font-size: 28px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.welcome-subtitle {
  margin: 4px 0 0;
  font-size: 14px;
  color: var(--color-text-secondary);
}

.project-picker-body {
  width: 100%;
  max-width: 560px;
  background: var(--color-surface);
  border: 1px solid var(--color-border);
  border-radius: 12px;
  padding: 20px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.current-project-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
}

.current-project-row .label {
  color: var(--color-text-secondary);
  flex-shrink: 0;
}

.path-display {
  background: var(--color-hover);
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 12px;
  font-family: monospace;
  color: var(--color-text-primary);
  word-break: break-all;
  max-width: 100%;
}

.input-row {
  width: 100%;
}

.path-input {
  width: 100%;
  padding: 10px 14px;
  border-radius: 8px;
  border: 1px solid var(--color-border);
  background: var(--color-input);
  color: var(--color-text-primary);
  font-size: 14px;
  font-family: monospace;
}

.path-input:focus {
  outline: none;
  border-color: var(--color-accent);
}

.actions-row {
  display: flex;
  gap: 8px;
}

.btn-primary,
.btn-secondary {
  padding: 8px 16px;
  border-radius: 8px;
  font-size: 13px;
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 6px;
  transition: opacity 0.15s;
}

.btn-primary {
  background: var(--color-accent);
  color: #fff;
  border: none;
  flex: 1;
}

.btn-primary:hover:not(:disabled) {
  opacity: 0.9;
}

.btn-secondary {
  background: transparent;
  color: var(--color-text-secondary);
  border: 1px solid var(--color-border);
}

.btn-secondary:hover:not(:disabled) {
  background: var(--color-hover);
}

.btn-primary:disabled,
.btn-secondary:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.error-msg {
  color: var(--color-danger);
  font-size: 13px;
  margin: 0;
  padding: 8px;
  background: color-mix(in srgb, var(--color-danger) 10%, transparent);
  border-radius: 6px;
}

.hint {
  font-size: 11px;
  color: var(--color-text-muted);
  margin: 0;
}

.hint code {
  font-family: monospace;
  background: var(--color-hover);
  padding: 1px 5px;
  border-radius: 3px;
}

.current-model {
  font-size: 12px;
  color: var(--color-text-secondary);
  margin-top: 4px;
}

.first-run-banner {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  border: 1px solid color-mix(in srgb, var(--color-accent) 40%, transparent);
  background: color-mix(in srgb, var(--color-accent) 8%, transparent);
  border-radius: 10px;
}

.banner-text {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 13px;
}

.banner-text strong {
  color: var(--color-text-primary);
}

.banner-text span {
  color: var(--color-text-secondary);
  font-size: 12px;
}

.link-btn {
  background: none;
  border: none;
  color: var(--color-accent);
  cursor: pointer;
  font-size: 12px;
  padding: 0 4px;
  text-decoration: underline;
}
</style>
