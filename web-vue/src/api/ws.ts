import type { ServerEvent, ClientMessage } from '@/types'

type EventHandler = (event: ServerEvent) => void
type ConnectionHandler = (connected: boolean) => void

export class ChatWebSocket {
  private ws: WebSocket | null = null
  private url: string
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private reconnectAttempts = 0
  private maxReconnectAttempts = 10
  private intentionalClose = false
  private eventHandler: EventHandler | null = null
  private connectionHandler: ConnectionHandler | null = null

  constructor(projectDir?: string) {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    const host = location.host
    const params = projectDir ? `?project_dir=${encodeURIComponent(projectDir)}` : ''
    this.url = `${proto}//${host}/ws/chat${params}`
  }

  onEvent(handler: EventHandler): void {
    this.eventHandler = handler
  }

  onConnectionChange(handler: ConnectionHandler): void {
    this.connectionHandler = handler
  }

  connect(): void {
    if (this.ws?.readyState === WebSocket.OPEN) return
    this.intentionalClose = false

    try {
      this.ws = new WebSocket(this.url)

      this.ws.onopen = () => {
        console.log('[WS] 已连接')
        this.reconnectAttempts = 0
        this.connectionHandler?.(true)
      }

      this.ws.onmessage = (evt) => {
        try {
          const event: ServerEvent = JSON.parse(evt.data)
          this.eventHandler?.(event)
        } catch {
          console.warn('[WS] 无法解析消息:', evt.data)
        }
      }

      this.ws.onclose = () => {
        console.log('[WS] 连接关闭')
        this.connectionHandler?.(false)
        if (!this.intentionalClose) {
          this.scheduleReconnect()
        }
      }

      this.ws.onerror = (err) => {
        console.error('[WS] 错误:', err)
      }
    } catch (err) {
      console.error('[WS] 创建连接失败:', err)
      this.scheduleReconnect()
    }
  }

  disconnect(): void {
    this.intentionalClose = true
    this.clearReconnect()
    this.ws?.close()
    this.ws = null
    this.connectionHandler?.(false)
  }

  send(msg: ClientMessage): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg))
    } else {
      console.warn('[WS] 未连接，无法发送消息')
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      console.error('[WS] 重连次数已达上限')
      return
    }
    this.clearReconnect()
    const delay = Math.min(1000 * 2 ** this.reconnectAttempts, 30000)
    this.reconnectAttempts++
    console.log(`[WS] ${delay}ms 后第 ${this.reconnectAttempts} 次重连`)
    this.reconnectTimer = setTimeout(() => this.connect(), delay)
  }

  private clearReconnect(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
  }
}