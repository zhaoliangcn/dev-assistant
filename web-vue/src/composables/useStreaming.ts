import { ref } from 'vue'

/**
 * 流式增量缓冲 + 节流渲染。
 *
 * 用于处理 WebSocket 流式 delta 事件：积攒增量文本，按固定间隔
 * 节流刷新到目标，避免逐 token 触发响应式更新导致性能问题。
 *
 * @param onFlush 节流到期时的回调，接收累积的增量文本
 * @param interval 节流间隔（ms），默认 50
 */
export function useStreaming(
  onFlush: (chunk: string) => void,
  interval = 50,
) {
  const streaming = ref(false)
  let buf = ''
  let timer: ReturnType<typeof setTimeout> | null = null

  /** 追加一段增量文本 */
  function push(delta: string) {
    if (!delta) return
    buf += delta
    streaming.value = true
    if (!timer) {
      timer = setTimeout(() => {
        flush()
      }, interval)
    }
  }

  /** 立即将缓冲区内容刷出 */
  function flush() {
    if (timer) {
      clearTimeout(timer)
      timer = null
    }
    if (buf) {
      onFlush(buf)
      buf = ''
    }
    streaming.value = false
  }

  /** 重置缓冲区（开始新一轮流式前调用） */
  function reset() {
    if (timer) {
      clearTimeout(timer)
      timer = null
    }
    buf = ''
    streaming.value = false
  }

  return { streaming, push, flush, reset }
}
