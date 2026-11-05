<script setup lang="ts">
import { RouterLink, useRoute } from 'vue-router'
import Icon from './Icon.vue'
const route = useRoute()
const items = [
  { key: 'market', to: '/', label: '行情', icon: 'market' },
  { key: 'watch', to: '/watch', label: '自选', icon: 'watch' },
  { key: 'ipo', to: '/ipo', label: '新股', icon: 'ipo' },
  { key: 'news', to: '/news', label: '资讯', icon: 'news' },
  { key: 'me', to: '/me', label: '我的', icon: 'me' },
] as const
</script>

<template>
  <nav class="bottom" aria-label="主导航">
    <RouterLink v-for="n in items" :key="n.key" :to="n.to" :aria-current="route.meta.tab === n.key || (n.key === 'me' && route.meta.tab === 'rank') ? 'page' : undefined">
      <Icon :name="n.icon" />
      <span>{{ n.label }}</span>
    </RouterLink>
  </nav>
</template>

<style scoped>
.bottom {
  position: fixed; left: 0; right: 0; bottom: 0; z-index: 30;
  height: calc(var(--nav-h) + env(safe-area-inset-bottom, 0px)); padding-bottom: env(safe-area-inset-bottom, 0px);
  background: var(--ground); border-top: 1px solid var(--rule);
  display: grid; grid-template-columns: repeat(5, minmax(0, 1fr));
}
a { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 3px; font-size: 11px; color: var(--ink-2); }
a[aria-current="page"] { color: var(--brand); }
@media (min-width: 1024px) { .bottom { display: none; } }
</style>
