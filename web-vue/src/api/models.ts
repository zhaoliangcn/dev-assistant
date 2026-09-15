import { get, post, del } from './client'
import type { ModelInfo, ModelsResponse, SystemStatus } from '@/types'

export async function getStatus(): Promise<SystemStatus> {
  return get<SystemStatus>('/status')
}

export async function listModels(): Promise<ModelsResponse> {
  return get<ModelsResponse>('/models')
}

export async function saveModel(config: {
  name: string
  provider: string
  api_url: string
  api_key?: string
  clear_api_key?: boolean
  model: string
  temperature?: number
  max_output_tokens?: number
  reasoning_effort?: string
}): Promise<void> {
  await post('/models', config)
}

export async function deleteModel(name: string): Promise<void> {
  await del(`/models/${encodeURIComponent(name)}`)
}

export async function switchModel(name: string): Promise<void> {
  await post('/models/switch', { name })
}
