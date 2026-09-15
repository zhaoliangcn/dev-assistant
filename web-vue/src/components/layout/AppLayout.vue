<script setup lang="ts"></script>

<template>
  <div class="app-layout">
    <header class="app-header">
      <slot name="header" />
    </header>
    <aside class="app-sidebar" v-show="$slots.sidebar">
      <slot name="sidebar" />
    </aside>
    <main class="app-main">
      <slot />
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
  overflow-y: auto;
}

.app-main {
  grid-area: main;
  overflow: hidden;
  display: flex;
  flex-direction: column;
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