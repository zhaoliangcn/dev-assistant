import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { useDark, useToggle } from '@vueuse/core'

export type Locale = 'zh' | 'en'

export const useAppStore = defineStore('app', () => {
  const isDark = useDark({
    storageKey: 'dev-assistant-vue-theme',
    valueDark: 'dark',
    valueLight: 'light',
  })
  const toggleTheme = useToggle(isDark)

  const sidebarOpen = ref(true)
  const sidebarTab = ref<'sessions' | 'files'>('sessions')

  const locale = ref<Locale>(
    (localStorage.getItem('dev-assistant-vue-locale') as Locale) || 'zh',
  )

  function toggleSidebar() {
    sidebarOpen.value = !sidebarOpen.value
  }

  function setSidebarTab(tab: 'sessions' | 'files') {
    sidebarTab.value = tab
  }

  function setLocale(l: Locale) {
    locale.value = l
    localStorage.setItem('dev-assistant-vue-locale', l)
  }

  watch(isDark, (dark) => {
    document.documentElement.classList.toggle('dark', dark)
  })

  return {
    isDark,
    toggleTheme,
    sidebarOpen,
    sidebarTab,
    locale,
    toggleSidebar,
    setSidebarTab,
    setLocale,
  }
})
