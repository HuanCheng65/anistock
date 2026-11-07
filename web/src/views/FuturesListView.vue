<script setup lang="ts">
// 行情 · 热度期货: every contract trading, one per work (docs/v3-futures.md §4.1), with the market's expectation
// of its period average and the day's change. The 股票 / 热度期货 switch sits above both lists.
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import ChangePill from '../components/ChangePill.vue'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import { usePoll } from '../composables/usePoll'
import { num, qty } from '../format'
import { useFutures } from '../stores/futures'
import { useSession } from '../stores/session'

const fut = useFutures()
const session = useSession()
const err = ref(false)
usePoll(() => fut.refreshList().then(() => (err.value = false), () => (err.value = true)), 10000, 30000)
usePoll(() => (session.me ? fut.refreshMine() : null), 30000, 60000)

type Sort = 'volume' | 'change' | 'left' | 'mine'
const SORTS: { key: Sort; label: string }[] = [
  { key: 'volume', label: '最热闹' },
  { key: 'change', label: '预期变化' },
  { key: 'left', label: '快到期' },
  { key: 'mine', label: '我持有的' },
]
const sort = ref<Sort>('volume')
const held = computed(() => new Set((fut.mine?.positions ?? []).filter((p) => p.quantity).map((p) => p.contract.id)))
const rows = computed(() => {
  const xs = (fut.list ?? []).filter((c) => c.status !== 'settled' && (sort.value !== 'mine' || held.value.has(c.id)))
  const left = (c: (typeof xs)[number]) => c.days_total - c.days_published
  const key: Record<Sort, (c: (typeof xs)[number]) => number> = {
    volume: (c) => -c.volume_today - c.open_interest / 1000,
    change: (c) => -Math.abs(c.change),
    left: left,
    mine: left,
  }
  return [...xs].sort((a, b) => key[sort.value](a) - key[sort.value](b))
})
</script>

<template>
  <main class="page futlist">
    <header class="pagehead phone-only"><h1>行情</h1></header>
    <div class="container">
      <h1 class="pagetitle desk-only">行情</h1>
      <nav class="seg" aria-label="品种">
        <RouterLink to="/stocks">股票</RouterLink>
        <span aria-current="page">热度期货</span>
      </nav>
      <div class="chips" role="group" aria-label="排序">
        <button v-for="s in SORTS" :key="s.key" type="button" :aria-pressed="sort === s.key" @click="sort = s.key">{{ s.label }}</button>
      </div>
      <EmptyState v-if="err && !fut.list" text="无法加载合约" action="重试" tone="error" @act="fut.refreshList()" />
      <EmptyState v-else-if="fut.list && !fut.list.length" text="热度期货还没有开放" action="去看股票" to="/stocks" />
      <template v-else-if="fut.list">
        <div class="head"><span>{{ rows.length }} 个合约</span><span>市场预期</span></div>
        <RouterLink v-for="c in rows" :key="c.id" :to="`/futures/${c.id}`" class="row">
          <Cover :id="c.stock.id" :title="c.stock.title" :w="30" :h="42" :spine="false" />
          <span class="nm">
            <span class="serif clamp1 t">{{ c.stock.title }}</span>
            <span class="sub">第 {{ c.period }} 期<template v-if="c.average_so_far != null"> · 已公布平均 <span class="num">{{ num(c.average_so_far) }}</span></template> · {{ c.status === 'expiring' ? '等待结算' : `还剩 ${c.days_total - c.days_published} 天` }}<template v-if="held.has(c.id)"> · 持有</template></span>
          </span>
          <span class="px">
            <span v-flash="c.last_price" class="num v">{{ num(c.last_price / 100) }}</span>
            <span class="num vol">{{ qty(c.volume_today) }} 手</span>
          </span>
          <ChangePill :value="c.change" />
        </RouterLink>
      </template>
      <div v-else><div v-for="i in 6" :key="i" class="skel" style="height: 52px; margin: 10px 0"></div></div>
    </div>
  </main>
</template>

<style scoped>
.seg { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); border: 1px solid var(--rule); margin-top: 4px; max-width: 360px; }
.seg a, .seg span { height: 36px; display: flex; align-items: center; justify-content: center; font-size: 14px; color: var(--ink-2); }
.seg [aria-current] { background: var(--ink); color: var(--ground); font-weight: 700; }
.chips { display: flex; gap: 8px; padding: 12px 0; overflow-x: auto; scrollbar-width: none; }
.chips button { flex: none; height: 32px; padding: 0 12px; font-size: 13px; border: 1px solid var(--base-line); background: transparent; color: var(--ink); }
.chips button[aria-pressed="true"] { background: var(--ink); color: var(--ground); border-color: var(--ink); }
.head { display: flex; justify-content: space-between; padding: 6px 0; font-size: 12px; color: var(--muted); border-bottom: 1px solid var(--rule); }
.row { display: grid; grid-template-columns: 30px minmax(0, 1fr) auto 84px; gap: 12px; align-items: center; padding: 10px 0; border-bottom: 1px solid var(--hair-2); color: var(--ink); }
.nm { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.t { font-size: 15px; font-weight: 600; }
.sub { font-size: 12px; color: var(--muted); }
.px { display: flex; flex-direction: column; align-items: flex-end; gap: 1px; }
.v { font-size: 15px; }
.vol { font-size: 11px; color: var(--muted); }
.row :deep(.pill) { min-width: 0; width: 84px; }
@media (min-width: 1024px) {
  .row { grid-template-columns: 30px minmax(0, 1fr) 140px 96px; padding: 12px 0; }
  .chips { flex-wrap: wrap; }
}
</style>
