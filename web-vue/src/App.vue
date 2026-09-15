<script setup lang="ts">
import AppHeader from '@/components/layout/AppHeader.vue'
import AppSidebar from '@/components/sidebar/AppSidebar.vue'
</script>

<template>
  <div class="app-layout">
    <header class="app-header">
      <AppHeader />
    </header>
    <aside class="app-sidebar">
      <AppSidebar />
    </aside>
    <main class="app-main">
      <router-view />
    </main>
  </div>
</template>

<style scoped>
.app-layout {
  display: grid;
  grid-template-columns: 280px 1fr;
  grid-template-rows: 48px 1fr;
  grid-template-areas:
    'header header'
    'sidebar main';
  height: 100vh;
  height: 100dvh;
  overflow: hidden;
}

.app-header {
  grid-area: header;
  background: var(--color-surface);
  border-bottom: 1px solid var(--color-border);
  z-index: 10;
}

.app-sidebar {
  grid-area: sidebar;
  background: var(--color-surface);
  border-right: 1px solid var(--color-border);
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.app-main {
  grid-area: main;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  background: var(--color-bg);
}

@media (max-width: 768px) {
  .app-layout {
    grid-template-columns: 1fr;
    grid-template-areas:
      'header'
      'main';
  }
  .app-sidebar {
    display: none;
  }
}
</style>