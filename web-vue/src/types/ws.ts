/** 服务端 → 客户端 WebSocket 事件类型 */
export type ServerEventType =
  | 'session_ready'
  | 'thinking'
  | 'tool_call'
  | 'tool_result'
  | 'assistant_message'
  | 'assistant_stream_delta'
  | 'reasoning_delta'
  | 'error'
  | 'done'
  | 'status'
  | 'token_usage'

/** 服务端事件 */
export type ServerEvent =
  | { type: 'session_ready'; session_id: string }
  | { type: 'thinking'; content: string }
  | { type: 'tool_call'; tool_name: string; args: Record<string, unknown> }
  | { type: 'tool_result'; tool_name: string; success: boolean; content: string }
  | { type: 'assistant_message'; content: string; streaming: boolean }
  | { type: 'assistant_stream_delta'; delta: string; is_final: boolean }
  | { type: 'reasoning_delta'; delta: string; is_final: boolean }
  | { type: 'error'; content: string }
  | { type: 'done'; message_id: string }
  | { type: 'status'; content: string }
  | { type: 'token_usage'; prompt_tokens: number; completion_tokens: number; total_tokens: number }

/** 客户端 → 服务端 消息 */
export type ClientMessage =
  | { type: 'user_message'; content: string; id: string }
  | { type: 'cancel'; message_id: string }