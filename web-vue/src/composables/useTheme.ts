import { computed } from 'vue'
import { useAppStore } from '@/stores/app'

/**
 * 主题切换 composable。
 *
 * 封装 app store 中的主题逻辑，提供组件级便捷接口。
 * 底层基于 @vueuse/core 的 useDark，持久化到 localStorage。
 */
export function useTheme() {
  const app = useAppStore()

  const isDark = computed(() => app.isDark)

  function toggle() {
    app.toggleTheme()
  }

  function setDark(dark: boolean) {
    if (app.isDark !== dark) {
      app.toggleTheme()
    }
  }

  return { isDark, toggle, setDark }
}
