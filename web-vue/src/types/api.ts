/** Token 用量 */
export interface TokenUsage {
  prompt_tokens: number
  completion_tokens: number
  total_tokens: number
}

/** 系统状态（对应后端 SystemStatus） */
export interface SystemStatus {
  version: string
  project_dir: string
  mode: string
  active_model: string
  online: boolean
  uptime: string
}

/** 模型信息（对应后端 ModelInfo，GET /api/models 返回） */
export interface ModelInfo {
  name: string
  provider: string
  api_url: string
  /** 脱敏后的密钥（****abcd），未设置为 null */
  api_key: string | null
  has_api_key: boolean
  model: string
  temperature: number | null
  max_output_tokens: number | null
  reasoning_effort: string | null
  active: boolean
}

/** GET /api/models 响应 */
export interface ModelsResponse {
  models: ModelInfo[]
  config_path: string
}
