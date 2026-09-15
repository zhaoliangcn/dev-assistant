import { defineStore } from 'pinia'
import { ref, shallowRef } from 'vue'
import type { ChatMessage, ToolMessage, ServerEvent } from '@/types'
import { ChatWebSocket } from '@/api/ws'
import { useSessionsStore } from './sessions'
import { useFilesStore } from './files'

let wsInstance: ChatWebSocket | null = null

function genId(): string {
  return `msg_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`
}

function createEmptyMessage(role: ChatMessage['role'], content = ''): ChatMessage {
  return {
    id: genId(),
    role,
    content,
    status: role === 'assistant' ? 'pending' : 'done',
    reasoning: '',
    toolCalls: [],
    timestamp: Date.now(),
  }
}

function initWs(projectDir: string | undefined, onEvent: (e: ServerEvent) => void, onConnection: (c: boolean) => void) {
  // 每次都创建新实例（支持 project_dir 切换）
  wsInstance = new ChatWebSocket(projectDir)
  wsInstance.onEvent(onEvent)
  wsInstance.onConnectionChange(onConnection)
  return wsInstance
}

export const useChatStore = defineStore('chat', () => {
  const messages = shallowRef<ChatMessage[]>([])
  const toolMessages = ref<ToolMessage[]>([])
  const sessionId = ref<string | null>(null)
  const busy = ref(false)
  const reasoningText = ref('')
  const reasoningActive = ref(false)
  const connected = ref(false)
  const projectDir = ref<string>('')
  const projectError = ref<string>('')
  const switchingProject = ref(false)

  let _ws: ChatWebSocket | null = null
  let _streamBuf = ''
  let _streamTimer: ReturnType<typeof setTimeout> | null = null
  let _streamTarget: ChatMessage | null = null
  let _pendingToolCall: ToolMessage | null = null
  let _reasoningBuf = ''
  let _reasoningTimer: ReturnType<typeof setTimeout> | null = null
  let _reasoningAutoHide: ReturnType<typeof setTimeout> | null = null
  let _switching = false

  function _flushStream() {
    if (_streamBuf && _streamTarget) {
      _streamTarget.content += _streamBuf
      _streamTarget.status = 'streaming'
      _streamBuf = ''
      // 触发响应式更新
      messages.value = [...messages.value]
    }
  }

  /** 140 字符滚动窗口：超长时截尾并加省略号前缀（对齐旧前端） */
  function _reasoningWindow(text: string): string {
    const compact = text.replace(/\s+/g, ' ').trim()
    if (compact.length <= 140) return compact
    return '…' + compact.slice(-140)
  }

  function _flushReasoning(isFinal = false) {
    if (_reasoningTimer) {
      clearTimeout(_reasoningTimer)
      _reasoningTimer = null
    }
    const full = _reasoningBuf
    if (isFinal) {
      _reasoningBuf = ''
      if (full) {
        // 结算：保留尾部让用户瞥见最终思路，1.2s 后收起
        reasoningText.value = _reasoningWindow(full)
        reasoningActive.value = true
        if (_reasoningAutoHide) clearTimeout(_reasoningAutoHide)
        _reasoningAutoHide = setTimeout(() => {
          reasoningText.value = ''
          reasoningActive.value = false
        }, 1200)
      }
      return
    }
    if (!full) return
    reasoningActive.value = true
    reasoningText.value = _reasoningWindow(full)
  }

  function handleServerEvent(event: ServerEvent) {
    switch (event.type) {
      case 'session_ready':
        sessionId.value = event.session_id
        break

      case 'assistant_message': {
        // 非流式完整消息
        const last = _findLastAssistant()
        if (last && last.status === 'pending') {
          last.content = event.content
          last.status = 'done'
        } else {
          const msg = createEmptyMessage('assistant', event.content)
          msg.status = 'done'
          messages.value = [...messages.value, msg]
        }
        break
      }

      case 'assistant_stream_delta': {
        _streamTarget = _findLastAssistant()
        if (!_streamTarget) {
          _streamTarget = createEmptyMessage('assistant')
          messages.value = [...messages.value, _streamTarget]
        }
        _streamTarget.status = 'streaming'
        _streamBuf += event.delta
        if (event.is_final) {
          _flushStream()
          _streamTarget.status = 'done'
          _streamTarget = null
        } else {
          // 50ms 节流刷新
          if (!_streamTimer) {
            _streamTimer = setTimeout(() => {
              _flushStream()
              _streamTimer = null
            }, 50)
          }
        }
        messages.value = [...messages.value]
        break
      }

      case 'reasoning_delta': {
        // 取消自动隐藏定时器（新的思考流开始）
        if (_reasoningAutoHide) {
          clearTimeout(_reasoningAutoHide)
          _reasoningAutoHide = null
        }
        reasoningActive.value = true
        _reasoningBuf += event.delta
        if (event.is_final) {
          _flushReasoning(true)
        } else if (!_reasoningTimer) {
          _reasoningTimer = setTimeout(() => {
            _flushReasoning(false)
          }, 80)
        }
        break
      }

      case 'tool_call': {
        _pendingToolCall = {
          id: genId(),
          toolName: event.tool_name,
          args: event.args,
          status: 'running',
          timestamp: Date.now(),
        }
        toolMessages.value = [...toolMessages.value, _pendingToolCall]
        // 同时在最后一条 assistant 消息上记录
        const last = _findLastAssistant()
        if (last) {
          last.toolCalls = [
            ...(last.toolCalls || []),
            { toolName: event.tool_name, args: event.args, pending: true },
          ]
        }
        break
      }

      case 'tool_result': {
        if (_pendingToolCall && _pendingToolCall.toolName === event.tool_name) {
          _pendingToolCall.result = event.content
          _pendingToolCall.success = event.success
          _pendingToolCall.status = event.success ? 'done' : 'error'
        }
        toolMessages.value = [...toolMessages.value]
        // 更新 assistant 消息上的工具调用
        const last = _findLastAssistant()
        if (last) {
          const idx = last.toolCalls.findIndex(t => t.toolName === event.tool_name && t.pending)
          if (idx >= 0) {
            last.toolCalls[idx] = {
              ...last.toolCalls[idx],
              result: event.content,
              success: event.success,
              pending: false,
            }
          }
        }
        _pendingToolCall = null
        break
      }

      case 'thinking':
        // 重置缓冲：thinking 是一次性状态消息，与 reasoning_delta 互不干扰
        _reasoningBuf = ''
        reasoningText.value = event.content
        reasoningActive.value = true
        break

      case 'done':
        busy.value = false
        _flushStream()
        _flushReasoning(true)
        break

      case 'error':
        busy.value = false
        messages.value = [
          ...messages.value,
          createEmptyMessage('error', event.content),
        ]
        break

      case 'token_usage': {
        const last = _findLastAssistant()
        if (last) {
          last.tokenUsage = {
            prompt_tokens: event.prompt_tokens,
            completion_tokens: event.completion_tokens,
            total_tokens: event.total_tokens,
          }
        }
        break
      }

      case 'status':
        console.log('[ChatStore] 状态:', event.content)
        break
    }
  }

  function _findLastAssistant(): ChatMessage | null {
    const list = messages.value
    for (let i = list.length - 1; i >= 0; i--) {
      if (list[i].role === 'assistant') return list[i]
    }
    return null
  }

  function _onConnection(c: boolean) {
    connected.value = c
    if (c && _switching) {
      // 项目切换后，刷新会话列表
      _switching = false
      switchingProject.value = false
      const sessions = useSessionsStore()
      const files = useFilesStore()
      sessions.load()
      files.browse()
    }
  }

  function connectWs(dir?: string) {
    if (dir !== undefined) {
      projectDir.value = dir
    }
    disconnectWs()
    _ws = initWs(
      projectDir.value || undefined,
      handleServerEvent,
      _onConnection,
    )
    _ws.connect()
  }

  function disconnectWs() {
    _ws?.disconnect()
    _ws = null
  }

  async function switchProject(dir: string) {
    if (!dir || dir === projectDir.value) return
    projectError.value = ''
    switchingProject.value = true
    _switching = true

    // 清空旧消息
    messages.value = []
    toolMessages.value = []
    sessionId.value = null
    reasoningText.value = ''
    reasoningActive.value = false
    busy.value = false

    projectDir.value = dir
    connectWs(dir)

    // 若 2s 内未连上，视为后端拒绝路径
    setTimeout(() => {
      if (_switching) {
        _switching = false
        switchingProject.value = false
        projectError.value = '项目目录无效或超出服务器工作目录范围'
      }
    }, 2000)
  }

  function sendMessage(content: string) {
    if (busy.value || !_ws) return

    _flushStream()
    _flushReasoning()
    reasoningText.value = ''
    reasoningActive.value = false

    const userMsg = createEmptyMessage('user', content)
    messages.value = [...messages.value, userMsg]
    busy.value = true
    toolMessages.value = []

    _ws.send({
      type: 'user_message',
      content,
      id: userMsg.id,
    })
  }

  function stopGeneration() {
    const last = _findLastAssistant()
    if (last && (last.status === 'streaming' || last.status === 'pending')) {
      _flushStream()
      _flushReasoning()
      if (last.status === 'streaming') {
        last.status = 'cancelled'
      }
      if (_pendingToolCall) {
        _pendingToolCall.status = 'error'
        _pendingToolCall.result = '已取消'
      }
      toolMessages.value = [...toolMessages.value]
    }
    busy.value = false
    if (_ws && last) {
      _ws.send({ type: 'cancel', message_id: last.id })
    }
  }

  function newChat() {
    _flushStream()
    _flushReasoning()
    disconnectWs()
    messages.value = []
    toolMessages.value = []
    sessionId.value = null
    reasoningText.value = ''
    reasoningActive.value = false
    busy.value = false
    projectError.value = ''
    connectWs()
  }

  function loadMessages(msgs: ChatMessage[]) {
    messages.value = msgs
  }

  function setSessionId(id: string) {
    sessionId.value = id
  }

  return {
    messages,
    toolMessages,
    sessionId,
    busy,
    reasoningText,
    reasoningActive,
    connected,
    projectDir,
    projectError,
    switchingProject,
    connectWs,
    disconnectWs,
    sendMessage,
    stopGeneration,
    newChat,
    switchProject,
    loadMessages,
    setSessionId,
    handleServerEvent,
  }
})