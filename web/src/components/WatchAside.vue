<script setup lang="ts">
// Desktop left column: 自选 / 持仓 / 涨幅.
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { money, pct, dir } from '../format'
import { useAccount } from '../stores/account'
import { useMarket } from '../stores/market'
import { usePrefs } from '../stores/prefs'
import { useSession } from '../stores/session'
import Tabs from './Tabs.vue'

const props = defineProps<{ current?: number }>()
const market = useMarket()
const prefs = usePrefs()
const account = useAccount()
const session = useSession()
const tab = ref(prefs.watchlist.length ? 'watch' : 'gain')
const tabs = computed(() => [
  { key: 'watch', label: '自选' },
  ...(session.me ? [{ key: 'hold', label: '持仓' }] : []),
  { key: 'gain', label: '涨幅' },
])
const rows = computed(() => {
  const all = market.stocks ?? []
  if (tab.value === 'watch') return prefs.watchlist.map((id) => market.byId.get(id)).filter((s) => !!s)
  if (tab.value === 'hold') {
    const ids = new Set((account.positions ?? []).map((p) => p.company_id))
    return all.filter((s) => ids.has(s.id))
  }
  return [...all].sort((a, b) => b.change - a.change).slice(0, 30)
})
</script>

<template>
  <aside class="aside" aria-label="自选">
    <Tabs v-model="tab" :items="tabs" label="列表" :size="13" class="tabs" />
    <RouterLink v-for="s in rows" :key="s!.id" :to="`/stock/${s!.id}`" class="w" :class="{ cur: s!.id === props.current }">
      <span class="clamp1 t">{{ s!.title }}</span>
      <span class="num px">{{ money(s!.price) }}</span>
      <span class="num code">{{ s!.code }}</span>
      <span class="num chg" :class="dir(s!.change)">{{ pct(s!.change) }}</span>
    </RouterLink>
    <p v-if="!rows.length" class="none">{{ tab === 'watch' ? '在股票页加入自选' : '暂无持仓' }}</p>
  </aside>
</template>

<style scoped>
.aside { display: flex; flex-direction: column; min-width: 0; }
.tabs { padding: 4px 16px 0; gap: 16px; }
.w { display: grid; grid-template-columns: minmax(0, 1fr) auto; column-gap: 10px; row-gap: 1px; padding: 8px 16px; border-bottom: 1px solid var(--hair-2); color: var(--ink); }
.w.cur { background: var(--ground-2); }
.w.cur .t { font-weight: 700; }
.t { font-size: 13px; }
.px { font-size: 13px; text-align: right; }
.code { font-size: 11px; color: var(--muted); }
.chg { font-size: 12px; text-align: right; }
.none { padding: 16px; font-size: 13px; color: var(--muted); }
</style>
