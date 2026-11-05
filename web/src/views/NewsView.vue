<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { ApiError, type NewsItem } from '../api'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import { usePoll } from '../composables/usePoll'
import { dayLabel, dir, hm, newsGroup, pct } from '../format'
import { useAccount } from '../stores/account'
import { useMarket } from '../stores/market'
import { useSession } from '../stores/session'

const market = useMarket()
const account = useAccount()
const session = useSession()
const chip = ref('all')
const error = ref('')
const { refresh } = usePoll(async () => {
  try {
    await market.refreshNews(150)
    error.value = ''
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '无法加载'
  }
}, 30000)
usePoll(() => Promise.all([market.stocks ? null : market.refreshQuotes(), session.me ? account.refreshPositions() : null]), 60000)

const held = computed(() => new Set((account.positions ?? []).map((p) => p.company_id)))
const chips = computed(() => {
  const groups = new Set((market.news ?? []).map((n) => newsGroup(n.kind).label))
  const out = [{ key: 'all', label: '全部' }]
  if (session.me && held.value.size) out.push({ key: 'mine', label: '持仓相关' })
  for (const g of ['快讯', '公告', '放送', '日结']) if (groups.has(g)) out.push({ key: g, label: g })
  return out
})
const items = computed(() =>
  (market.news ?? []).filter((n) => {
    if (chip.value === 'all') return true
    if (chip.value === 'mine') return n.company_id != null && held.value.has(n.company_id)
    return newsGroup(n.kind).label === chip.value
  }),
)
const days = computed(() => {
  const out: { day: string; items: NewsItem[] }[] = []
  for (const n of items.value) {
    const last = out[out.length - 1]
    if (last && last.day === n.day) last.items.push(n)
    else out.push({ day: n.day, items: [n] })
  }
  return out
})
</script>

<template>
  <main class="page newspage">
    <header class="pagehead phone-only"><h1>资讯</h1></header>
    <div class="container">
      <h1 class="pagetitle desk-only">资讯</h1>
      <div class="chips gutter" role="group" aria-label="类型">
        <button v-for="c in chips" :key="c.key" type="button" :aria-pressed="chip === c.key" @click="chip = c.key">{{ c.label }}</button>
      </div>
      <EmptyState v-if="error && !market.news" :text="error" action="重试" tone="error" @act="refresh" />
      <section v-for="d in days" :key="d.day">
        <h2 class="dayh gutter">{{ dayLabel(d.day, session.status?.day) }}</h2>
        <component :is="n.company_id ? RouterLink : 'div'" v-for="n in d.items" :key="n.id" :to="n.company_id ? `/stocks/${n.company_id}` : undefined" class="item gutter">
          <span class="num tm">{{ n.ts ? hm(n.ts) : '' }}</span>
          <span class="body">
            <span class="tags">
              <span class="kind" :class="newsGroup(n.kind).tone">{{ newsGroup(n.kind).label }}</span>
              <span v-if="n.company_id && held.has(n.company_id)" class="mine">持仓</span>
            </span>
            <span class="title">{{ n.title }}</span>
            <span v-if="n.body" class="text">{{ n.body }}</span>
            <span v-if="n.company_id && market.byId.get(n.company_id)" class="stock">
              <Cover :id="n.company_id" :title="market.byId.get(n.company_id)!.title" :w="14" :h="18" :spine="false" />
              <span class="clamp1">{{ market.byId.get(n.company_id)!.title }}</span>
              <span class="num" :class="dir(market.byId.get(n.company_id)!.change)">{{ pct(market.byId.get(n.company_id)!.change) }}</span>
            </span>
          </span>
        </component>
      </section>
      <EmptyState v-if="market.news && !items.length" text="暂无资讯" />
      <div v-if="!market.news && !error" class="gutter"><div v-for="i in 4" :key="i" class="skel" style="height: 60px; margin: 14px 0"></div></div>
    </div>
  </main>
</template>

<style scoped>
.chips { display: flex; gap: 8px; padding-top: 12px; padding-bottom: 12px; border-bottom: 1px solid var(--hair); overflow-x: auto; scrollbar-width: none; }
.chips button { flex: none; height: 30px; padding: 0 12px; font-size: 13px; border: 1px solid var(--base-line); background: transparent; color: var(--ink); }
.chips button[aria-pressed="true"] { background: var(--ink); color: var(--ground); border-color: var(--ink); }
.dayh { padding-top: 14px; padding-bottom: 6px; font-size: 13px; font-weight: 700; color: var(--ink-2); border-bottom: 1px solid var(--hair); }
.item { display: grid; grid-template-columns: 44px minmax(0, 1fr); column-gap: 10px; padding-top: 12px; padding-bottom: 12px; border-bottom: 1px solid var(--hair-2); color: var(--ink); }
.tm { font-size: 12px; color: var(--muted); padding-top: 3px; }
.body { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.tags { display: flex; align-items: center; gap: 8px; }
.kind { font-size: 11px; padding: 1px 6px; border: 1px solid currentColor; }
.kind.flash { color: var(--brand); }
.kind.notice { color: var(--ink); }
.kind.air { color: var(--mk-us); }
.kind.close { color: var(--ink-2); }
.mine { font-size: 11px; color: var(--brand); }
.title { font-size: 15px; line-height: 1.5; }
.text { font-size: 13px; color: var(--ink-2); line-height: 1.55; }
.stock { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--ink-2); min-width: 0; }
@media (min-width: 1024px) {
  .chips { padding-top: 0; }
  .item { grid-template-columns: 64px minmax(0, 1fr); padding-top: 14px; padding-bottom: 14px; }
  .body { display: grid; grid-template-columns: 72px minmax(0, 1fr) 320px; grid-template-areas: "tags title stock" "tags text stock"; column-gap: 20px; row-gap: 4px; align-items: start; }
  .tags { grid-area: tags; padding-top: 2px; flex-direction: column; align-items: flex-start; gap: 4px; }
  .title { grid-area: title; }
  .text { grid-area: text; }
  .stock { grid-area: stock; justify-self: end; max-width: 320px; }
}
</style>
