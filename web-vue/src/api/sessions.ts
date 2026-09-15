import { get, post, del } from './client'
import type { SessionSummary, SessionDetail, ChatMessage, SessionEvent } from '@/types'

export async function listSessions(): Promise<SessionSummary[]> {
  return get<SessionSummary[]>('/sessions')
}

export async function getSession(
  id: string,
  limit?: number,
): Promise<SessionDetail> {
  const query = limit ? { limit } : undefined
  return get<SessionDetail>(
    `/sessions/${encodeURIComponent(id)}`,
    query as Record<string, string | number> | undefined,
  )
}

export async function deleteSession(id: string): Promise<void> {
  await del(`/sessions/${encodeURIComponent(id)}`)
}

export async function renameSession(id: string, title: string): Promise<void> {
  await post(`/sessions/${encodeURIComponent(id)}/rename`, { title })
}

export async function exportSession(
  id: string,
  format: 'md' | 'json' = 'md',
): Promise<string> {
  const params = new URLSearchParams({ format })
  // export 返回原始文本，不走 JSON 解析
  const client = (await import('./client')).default
  return await client(`/sessions/${encodeURIComponent(id)}/export?${params}`, {
    headers: {
      Accept: format === 'json' ? 'application/json' : 'text/markdown',
    },
    responseType: 'text',
  })
}

/** 将后端 SessionEvent 列表转换为前端 ChatMessage 列表 */
export function toLocalMessages(detail: SessionDetail): ChatMessage[] {
  const messages: ChatMessage[] = []
  for (const ev of detail.events) {
    const ts = parseTimestamp(ev.timestamp)
    switch (ev.type) {
      case 'user_message':
        messages.push({
          id: `msg_${messages.length}`,
          role: 'user',
          content: ev.content,
          status: 'done',
          reasoning: '',
          toolCalls: [],
          timestamp: ts,
        })
        break
      case 'assistant_message':
        messages.push({
          id: `msg_${messages.length}`,
          role: 'assistant',
          content: ev.content,
          status: 'done',
          reasoning: '',
          toolCalls: [],
          timestamp: ts,
        })
        break
      case 'system_message':
        messages.push({
          id: `msg_${messages.length}`,
          role: 'system',
          content: ev.content,
          status: 'done',
          reasoning: '',
          toolCalls: [],
          timestamp: ts,
        })
        break
      // tool_call_request / tool_result / compression 不作为独立消息渲染
    }
  }
  return messages
}

function parseTimestamp(ts: string): number {
  const d = new Date(ts).getTime()
  return isNaN(d) ? Date.now() : d
}
