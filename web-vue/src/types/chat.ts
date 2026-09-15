import type { TokenUsage } from './api'

/** 消息角色 */
export type MessageRole = 'user' | 'assistant' | 'system' | 'error'

/** 消息状态 */
export type MessageStatus = 'pending' | 'streaming' | 'done' | 'error' | 'cancelled'

/** 单条消息（前端内部表示） */
export interface ChatMessage {
  id: string
  role: MessageRole
  content: string
  status: MessageStatus
  /** 思考过程文本 */
  reasoning: string
  /** 工具调用记录 */
  toolCalls: ToolCallRecord[]
  /** Token 用量（仅 assistant 消息） */
  tokenUsage?: TokenUsage
  /** 时间戳 */
  timestamp: number
}

/** 工具调用记录 */
export interface ToolCallRecord {
  toolName: string
  args: Record<string, unknown>
  result?: string
  success?: boolean
  pending: boolean
}

/** 工具活动消息（独立于对话上下文） */
export interface ToolMessage {
  id: string
  toolName: string
  args: Record<string, unknown>
  result?: string
  success?: boolean
  status: 'running' | 'done' | 'error'
  timestamp: number
}

// ---------------------------------------------------------------------------
// 后端持久化事件类型（对应 Rust SessionEvent，#[serde(tag="type")]）
// ---------------------------------------------------------------------------

export interface SessionEventBase {
  type: string
  timestamp: string
  session_id: string
}

export interface UserMessageEvent extends SessionEventBase {
  type: 'user_message'
  content: string
}

export interface AssistantMessageEvent extends SessionEventBase {
  type: 'assistant_message'
  content: string
}

export interface SystemMessageEvent extends SessionEventBase {
  type: 'system_message'
  content: string
}

export interface ToolCallRequestEvent extends SessionEventBase {
  type: 'tool_call_request'
  tool_call_id: string
  name: string
  arguments: unknown
}

export interface ToolResultEvent extends SessionEventBase {
  type: 'tool_result'
  tool_call_id: string
  name: string
  success: boolean
  content: string
}

export interface CompressionEvent extends SessionEventBase {
  type: 'compression'
  original_messages: number
  after_messages: number
  kept_rounds: number
  original_tokens: number
  after_tokens: number
}

export type SessionEvent =
  | UserMessageEvent
  | AssistantMessageEvent
  | SystemMessageEvent
  | ToolCallRequestEvent
  | ToolResultEvent
  | CompressionEvent

// ---------------------------------------------------------------------------
// 会话 / 文件 API 类型（对应后端 Rust 结构体）
// ---------------------------------------------------------------------------

/** 会话摘要（GET /api/sessions） */
export interface SessionSummary {
  id: string
  title: string
  message_count: number
  created_at: string
}

/** 会话详情（GET /api/sessions/{id}） */
export interface SessionDetail {
  id: string
  events: SessionEvent[]
  total: number
  returned: number
}

/** 文件条目（GET /api/files 内的 entries 元素） */
export interface FileEntry {
  name: string
  path: string
  is_dir: boolean
  size: number
}

/** 目录列表响应（GET /api/files） */
export interface ListFilesResponse {
  entries: FileEntry[]
  current_path: string
}

/** 文件内容响应（GET /api/files/content） */
export interface FileContent {
  path: string
  content: string
  total_lines: number
}

/** 导出格式 */
export type ExportFormat = 'md' | 'json'
