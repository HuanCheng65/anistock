<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { money, pct, dir } from '../format'
import { useMarket } from '../stores/market'
import { useSession } from '../stores/session'
import BrandMark from './BrandMark.vue'
import MarketClock from './MarketClock.vue'
import SearchPanel from './SearchPanel.vue'

const route = useRoute()
const market = useMarket()
const session = useSession()
const nav = [
  { key: 'market', to: '/', label: '行情' },
  { key: 'ipo', to: '/ipo', label: '新股' },
  { key: 'news', to: '/news', label: '资讯' },
  { key: 'rank', to: '/rank', label: '排行' },
  { key: 'me', to: '/me', label: '我的' },
]
const SHORT: Record<string, string> = { all: '综合', season: '新番', long: '长篇' }
const ticker = computed(() => (market.indices?.indices ?? []).map((i) => ({ k: i.key, label: SHORT[i.key], v: i.value, c: i.change })))
</script>

<template>
  <header class="desk">
    <RouterLink to="/" class="brand" aria-label="AniEx 首页">
      <BrandMark :size="28" />
      <span class="serif word">AniEx</span><span class="cn">安易</span>
    </RouterLink>
    <nav aria-label="主导航">
      <RouterLink v-for="n in nav" :key="n.key" :to="n.to" :aria-current="route.meta.tab === n.key || (n.key === 'market' && route.meta.tab === 'watch') ? 'page' : undefined">{{ n.label }}</RouterLink>
    </nav>
    <div class="ticker num">
      <span v-for="t in ticker" :key="t.k">{{ t.label }} <span class="v">{{ money(t.v) }}</span> <span :class="dir(t.c)">{{ pct(t.c) }}</span></span>
    </div>
    <SearchPanel compact class="search" />
    <MarketClock seconds />
    <RouterLink v-if="session.checked && !session.me" to="/login" class="who">登录</RouterLink>
    <RouterLink v-else-if="session.me" to="/me" class="who clamp1">{{ session.me.username }}</RouterLink>
  </header>
</template>

<style scoped>
.desk {
  display: none; position: sticky; top: 0; z-index: 30; background: var(--ground);
  align-items: center; gap: 20px; padding: 10px 20px; border-bottom: 1px solid var(--rule); min-height: var(--desk-header-h);
}
@media (min-width: 1024px) { .desk { display: flex; } }
.brand { display: flex; align-items: center; gap: 10px; color: var(--ink); }
.word { font-weight: 900; font-size: 19px; }
.cn { font-size: 13px; color: var(--ink-2); }
nav { display: flex; gap: 18px; font-size: 14px; }
nav a[aria-current="page"] { color: var(--brand); font-weight: 700; }
.ticker { display: flex; gap: 18px; font-size: 13px; color: var(--ink-2); white-space: nowrap; overflow: hidden; min-width: 0; }
.ticker .v { color: var(--ink); }
.search { margin-left: auto; width: 240px; flex: none; }
.who { font-size: 13px; max-width: 8em; color: var(--ink-2); }
@media (max-width: 1279px) { .ticker span:nth-child(n + 2) { display: none; } }
</style>
