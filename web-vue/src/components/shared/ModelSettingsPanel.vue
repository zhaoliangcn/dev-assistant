<script setup lang="ts">
import { ref } from 'vue'
import { useModelsStore } from '@/stores'
import { useI18n } from '@/composables/useI18n'
import type { ModelInfo } from '@/types'

const emit = defineEmits<{
  close: []
}>()

const models = useModelsStore()
const { t } = useI18n()

const editing = ref(false)
const editingModel = ref<ModelInfo | null>(null)
const saving = ref(false)
const error = ref('')

const form = ref({
  name: '',
  provider: '',
  api_url: '',
  api_key: '',
  clear_api_key: false,
  model: '',
  temperature: '' as string | number,
  max_output_tokens: '' as string | number,
  reasoning_effort: '' as string,
})

function startAdd() {
  editingModel.value = null
  form.value = {
    name: '',
    provider: 'openai',
    api_url: '',
    api_key: '',
    clear_api_key: false,
    model: '',
    temperature: '',
    max_output_tokens: '',
    reasoning_effort: '',
  }
  error.value = ''
  editing.value = true
}

function startEdit(m: ModelInfo) {
  editingModel.value = m
  form.value = {
    name: m.name,
    provider: m.provider,
    api_url: m.api_url,
    api_key: '',
    clear_api_key: false,
    model: m.model,
    temperature: m.temperature ?? '',
    max_output_tokens: m.max_output_tokens ?? '',
    reasoning_effort: m.reasoning_effort ?? '',
  }
  error.value = ''
  editing.value = true
}

async function save() {
  if (!form.value.name || !form.value.provider || !form.value.api_url || !form.value.model) {
    error.value = t('form_required')
    return
  }

  saving.value = true
  error.value = ''
  try {
    await models.save({
      name: form.value.name,
      provider: form.value.provider,
      api_url: form.value.api_url,
      api_key: form.value.api_key || undefined,
      clear_api_key: form.value.clear_api_key,
      model: form.value.model,
      temperature: form.value.temperature === '' ? undefined : Number(form.value.temperature),
      max_output_tokens: form.value.max_output_tokens === '' ? undefined : Number(form.value.max_output_tokens),
      reasoning_effort: form.value.reasoning_effort || undefined,
    })
    editing.value = false
  } catch (e) {
    error.value = (e as Error).message
  } finally {
    saving.value = false
  }
}

async function remove(name: string) {
  if (!confirm(t('delete_confirm') + ' ' + name + '?')) return
  try {
    await models.remove(name)
    if (editingModel.value?.name === name) {
      editing.value = false
    }
  } catch (e) {
    error.value = (e as Error).message
  }
}

function close() {
  editing.value = false
  error.value = ''
  emit('close')
}

const providers = ['openai', 'anthropic', 'ollama', 'deepseek', 'sensechat', 'kimi']
</script>

<template>
  <div class="settings-overlay" @click.self="emit('close')">
    <div class="settings-panel">
      <div class="settings-header">
        <strong>⚙️ {{ t('model_settings') }}</strong>
        <button class="close-btn" @click="close">✕</button>
      </div>

      <div class="settings-body">
        <p v-if="models.configPath" class="config-path">
          📄 {{ t('config_file') }}: {{ models.configPath }}
        </p>
        <p class="settings-hint">{{ t('model_settings_hint') }}</p>

        <div v-if="models.loading" class="loading">{{ t('saving') }}</div>

        <div v-if="!editing" class="model-list-section">
          <button class="add-btn" @click="startAdd">+ {{ t('add_model') }}</button>

          <div v-if="models.models.length === 0" class="empty">{{ t('no_models') }}</div>

          <div v-for="m in models.models" :key="m.name" :class="['model-item', { active: m.active }]">
            <div class="model-info">
              <div class="model-title">
                <strong>{{ m.name }}</strong>
                <span v-if="m.active" class="badge-active">{{ t('active') }}</span>
              </div>
              <small>{{ m.provider }} · {{ m.model }}</small>
              <small class="model-url">{{ m.api_url }}</small>
              <small :class="m.has_api_key ? 'key-ok' : 'key-missing'">
                {{ m.has_api_key ? '🔑 ' + t('api_key_set') : t('api_key_missing') }}
              </small>
            </div>
            <div class="model-actions">
              <button class="action-btn" @click="startEdit(m)" title="Edit">✏️</button>
              <button class="action-btn danger" @click="remove(m.name)" title="Delete">🗑</button>
            </div>
          </div>
        </div>

        <div v-else class="model-form">
          <h4>{{ editingModel ? t('edit_model') : t('add_model') }}</h4>

          <div class="form-row">
            <label>{{ t('model_name') }}</label>
            <input v-model="form.name" :placeholder="t('model_name_placeholder')" :disabled="!!editingModel" />
          </div>

          <div class="form-row">
            <label>{{ t('provider') }}</label>
            <select v-model="form.provider">
              <option v-for="p in providers" :key="p" :value="p">{{ p }}</option>
            </select>
          </div>

          <div class="form-row">
            <label>{{ t('api_url') }}</label>
            <input v-model="form.api_url" placeholder="https://api.openai.com/v1" />
          </div>

          <div class="form-row">
            <label>{{ t('api_key') }}</label>
            <input v-model="form.api_key" type="password" :placeholder="t('api_key_keep')" />
            <label class="checkbox-label" v-if="editingModel">
              <input type="checkbox" v-model="form.clear_api_key" />
              {{ t('clear_api_key') }}
            </label>
          </div>

          <div class="form-row">
            <label>{{ t('model_name') }}</label>
            <input v-model="form.model" placeholder="gpt-4o / deepseek-chat / ..." />
          </div>

          <div class="form-row two-col">
            <div>
              <label>{{ t('temperature') }}</label>
              <input v-model="form.temperature" type="number" step="0.1" min="0" max="2" placeholder="0.7" />
            </div>
            <div>
              <label>{{ t('max_output_tokens') }}</label>
              <input v-model="form.max_output_tokens" type="number" step="1" min="1" placeholder="4096" />
            </div>
          </div>

          <div class="form-row">
            <label>{{ t('reasoning_effort') }}</label>
            <select v-model="form.reasoning_effort">
              <option value="">— {{ t('reasoning_effort_hint') }} —</option>
              <option value="none">none</option>
              <option value="low">low</option>
              <option value="medium">medium</option>
              <option value="high">high</option>
            </select>
          </div>

          <p v-if="error" class="form-error">{{ error }}</p>

          <div class="form-actions">
            <button class="btn-cancel" @click="close">{{ t('cancel') }}</button>
            <button class="btn-save" @click="save" :disabled="saving">
              {{ saving ? t('saving') : t('save') }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}

.settings-panel {
  background: var(--color-surface);
  border-radius: 12px;
  width: 560px;
  max-width: 90vw;
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  border: 1px solid var(--color-border);
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.2);
}

.settings-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border-bottom: 1px solid var(--color-border);
}

.close-btn {
  border: none;
  background: transparent;
  font-size: 18px;
  cursor: pointer;
  color: var(--color-text-muted);
  padding: 4px 8px;
  border-radius: 4px;
}

.close-btn:hover {
  background: var(--color-hover);
}

.settings-body {
  padding: 14px 18px;
  overflow-y: auto;
  flex: 1;
}

.config-path {
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: monospace;
  margin-bottom: 8px;
  word-break: break-all;
}

.settings-hint {
  font-size: 12px;
  color: var(--color-text-secondary);
  margin-bottom: 14px;
}

.loading {
  text-align: center;
  padding: 24px;
  color: var(--color-text-muted);
}

.add-btn {
  width: 100%;
  padding: 8px;
  border: 1px dashed var(--color-border);
  border-radius: 8px;
  background: transparent;
  color: var(--color-accent);
  font-size: 13px;
  cursor: pointer;
  margin-bottom: 10px;
}

.add-btn:hover {
  background: var(--color-hover);
}

.empty {
  text-align: center;
  color: var(--color-text-muted);
  font-size: 13px;
  padding: 24px 0;
}

.model-item {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  padding: 10px 12px;
  border-radius: 8px;
  border: 1px solid var(--color-border);
  margin-bottom: 6px;
  gap: 8px;
}

.model-item.active {
  border-color: var(--color-accent);
  background: color-mix(in srgb, var(--color-accent) 8%, transparent);
}

.model-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  flex: 1;
}

.model-title {
  display: flex;
  align-items: center;
  gap: 6px;
}

.badge-active {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 8px;
  background: var(--color-accent);
  color: #fff;
}

.model-url {
  font-size: 10px;
  color: var(--color-text-muted);
  font-family: monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.key-ok {
  color: var(--color-success);
  font-size: 11px;
}

.key-missing {
  color: var(--color-danger);
  font-size: 11px;
}

.model-actions {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
}

.action-btn {
  border: none;
  background: transparent;
  cursor: pointer;
  font-size: 14px;
  padding: 4px 6px;
  border-radius: 4px;
}

.action-btn:hover {
  background: var(--color-hover);
}

.action-btn.danger:hover {
  background: color-mix(in srgb, var(--color-danger) 15%, transparent);
}

.model-form h4 {
  margin: 0 0 12px;
  font-size: 15px;
}

.form-row {
  margin-bottom: 10px;
}

.form-row label {
  display: block;
  font-size: 12px;
  color: var(--color-text-secondary);
  margin-bottom: 4px;
}

.form-row input,
.form-row select {
  width: 100%;
  padding: 6px 10px;
  border-radius: 6px;
  border: 1px solid var(--color-border);
  background: var(--color-input);
  color: var(--color-text-primary);
  font-size: 13px;
}

.form-row input:disabled {
  opacity: 0.5;
}

.two-col {
  display: flex;
  gap: 12px;
}

.two-col > div {
  flex: 1;
}

.checkbox-label {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-top: 4px;
  font-size: 11px;
  cursor: pointer;
}

.checkbox-label input {
  width: auto;
}

.form-error {
  color: var(--color-danger);
  font-size: 12px;
  margin: 8px 0;
}

.form-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 14px;
}

.btn-cancel,
.btn-save {
  padding: 6px 16px;
  border-radius: 6px;
  border: 1px solid var(--color-border);
  font-size: 13px;
  cursor: pointer;
}

.btn-cancel {
  background: transparent;
  color: var(--color-text-secondary);
}

.btn-save {
  background: var(--color-accent);
  color: #fff;
  border-color: var(--color-accent);
}

.btn-save:disabled {
  opacity: 0.5;
}
</style>
