import { defineStore } from 'pinia'
import { ref } from 'vue'
import type { ModelInfo, SystemStatus } from '@/types'
import { getStatus, listModels, saveModel, deleteModel, switchModel } from '@/api/models'

export const useModelsStore = defineStore('models', () => {
  const models = ref<ModelInfo[]>([])
  const activeModel = ref<string>('')
  const configPath = ref<string>('')
  const status = ref<SystemStatus | null>(null)
  const loading = ref(false)

  async function loadModels() {
    loading.value = true
    try {
      const [modelsRes, sysStatus] = await Promise.all([
        listModels().catch(() => ({ models: [] as ModelInfo[], config_path: '' })),
        getStatus().catch(() => null),
      ])
      models.value = modelsRes.models
      configPath.value = modelsRes.config_path
      status.value = sysStatus
      if (sysStatus) {
        activeModel.value = sysStatus.active_model
      } else {
        // 回退：从 models 中找 active=true
        const active = models.value.find(m => m.active)
        if (active) activeModel.value = active.name
      }
    } finally {
      loading.value = false
    }
  }

  async function switch_(name: string) {
    await switchModel(name)
    activeModel.value = name
    models.value = models.value.map(m => ({ ...m, active: m.name === name }))
  }

  async function save(config: {
    name: string
    provider: string
    api_url: string
    model: string
    api_key?: string
    clear_api_key?: boolean
    temperature?: number
    max_output_tokens?: number
    reasoning_effort?: string
  }) {
    await saveModel(config)
    await loadModels()
  }

  async function remove(name: string) {
    await deleteModel(name)
    await loadModels()
  }

  return { models, activeModel, configPath, status, loading, loadModels, switch: switch_, save, remove }
})
