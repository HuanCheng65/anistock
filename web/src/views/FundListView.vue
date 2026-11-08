<script setup lang="ts">
// 行情 · 基金: the index funds in three groups (市场 / 原作 / 题材), each with its net value, members, the last
// month and 今日估算 (the tracked index's change today).
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import ChangePill from '../components/ChangePill.vue'
import EmptyState from '../components/EmptyState.vue'
import MarketSeg from '../components/MarketSeg.vue'
import { usePoll } from '../composables/usePoll'
import { num, pct } from '../format'
import { useFunds } from '../stores/funds'
import { useSession } from '../stores/session'

const funds = useFunds()
const session = useSession()
const err = ref(false)
usePoll(() => funds.refreshList().then(() => (err.value = false), () => (err.value = true)), 15000, 60000)
usePoll(() => (session.me ? funds.refreshMine().catch(() => {}) : null), 30000, 60000)

const GROUPS = [
  { key: 'market', label: '市场' },
  { key: 'source', label: '原作' },
  { key: 'theme', label: '题材' },
]
const held = computed(() => new Set((funds.mine?.holdings ?? []).map((h) => h.fund.key)))
const groups = computed(() =>
  GROUPS.map((g) => ({ ...g, rows: (funds.list ?? []).filter((f) => f.group === g.key) })).filter((g) => g.rows.length),
)
</script>

<template>
  <main class="page fundlist">
    <header class="pagehead phone-only"><h1>行情</h1></header>
    <div class="container">
      <h1 class="pagetitle desk-only">行情</h1>
      <MarketSeg current="funds" />
      <EmptyState v-if="err && !funds.list" text="无法加载基金" action="重试" tone="error" @act="funds.refreshList()" />
      <EmptyState v-else-if="funds.list && !funds.list.length" text="指数基金还没有开放" action="去看股票" to="/stocks" />
      <template v-else-if="funds.list">
        <section v-for="g in groups" :key="g.key" class="grp">
          <div class="head"><span>{{ g.label }}</span><span>净值</span></div>
          <RouterLink v-for="f in g.rows" :key="f.key" :to="`/funds/${f.key}`" class="row">
            <span class="nm">
              <span class="t">{{ f.name }}</span>
              <span class="sub">{{ f.members }} 只<template v-if="f.month_change != null"> · 近一月 <span class="num">{{ pct(f.month_change) }}</span></template><template v-else> · 成立以来 <span class="num">{{ pct(f.since_start) }}</span></template><template v-if="held.has(f.key)"> · 持有</template></span>
            </span>
            <span class="num v">{{ num(f.nav, 4) }}</span>
            <ChangePill :value="f.estimate" />
          </RouterLink>
        </section>
        <p class="note">涨跌为今日估算，按跟踪指数的实时点位推算。净值每天收盘后公布。</p>
      </template>
      <div v-else><div v-for="i in 6" :key="i" class="skel" style="height: 52px; margin: 10px 0"></div></div>
    </div>
  </main>
</template>

<style scoped>
.grp { margin-top: 12px; }
.head { display: flex; justify-content: space-between; padding: 6px 0; font-size: 12px; color: var(--muted); border-bottom: 1px solid var(--rule); }
.row { display: grid; grid-template-columns: minmax(0, 1fr) auto 84px; gap: 12px; align-items: center; padding: 12px 0; border-bottom: 1px solid var(--hair-2); color: var(--ink); }
.nm { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.t { font-size: 15px; font-weight: 600; }
.sub { font-size: 12px; color: var(--muted); }
.v { font-size: 15px; }
.row :deep(.pill) { min-width: 0; width: 84px; }
.note { margin: 16px 0; font-size: 12px; color: var(--muted); line-height: 1.6; }
@media (min-width: 1024px) {
  .row { grid-template-columns: minmax(0, 1fr) 140px 96px; }
}
</style>
