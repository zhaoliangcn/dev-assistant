import { useChatStore } from '@/stores/chat'
import { onUnmounted } from 'vue'

export function useWebSocket() {
  const store = useChatStore()

  store.connectWs()

  onUnmounted(() => {
    store.disconnectWs()
  })

  return {
    connected: store.connected,
    send: store.sendMessage.bind(store),
    stop: store.stopGeneration.bind(store),
  }
}