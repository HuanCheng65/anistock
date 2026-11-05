<script setup lang="ts">
// 资讯: the four groups (the server's `category`) as filters, cursor paging ("加载更多"), new items by push.
import { computed, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { api, call, errorText, type NewsItem } from '../api'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import { usePoll } from '../composables/usePoll'
import { dayLabel, dir, hm, NEWS_CATEGORY, newsGroup, pct } from '../format'
import { onLive } from '../live'
import { useAccount } from '../stores/account'
import { useMarket } from '../stores/market'
import { useSession } from '../stores/session'
import { onBeforeUnmount } from 'vue'

const market = useMarket()
const account = useAccount()
const session = useSession()
const chip = ref('all')
const error = ref('')
const list = ref<NewsItem[] | null>(null)
const next = ref<string | null>(null)
const loading = ref(false)

/** the server-side filter of a chip: a category, or none ('all', and 'mine' filters what is loaded) */
const category = (c: string) => (c in NEWS_CATEGORY ? c : undefined)
async function load(more = false) {
  loading.value = true
  try {
    const r = await call(api.GET('/api/v1/news', {
      params: { query: { limit: 50, category: category(chip.value), before: more ? next.value ?? undefined : undefined } },
    }))
    list.value = more ? [...(list.value ?? []), ...r.items] : r.items
    next.value = r.next_cursor
    error.value = ''
  } catch (e) {
    error.value = errorText(e, '无法加载')
  } finally {
    loading.value = false
  }
}
watch(chip, () => {
  list.value = null
  load()
})
// a fallback while the socket is down: reload the first page only when nothing more was paged in
const { refresh } = usePoll(() => ((list.value?.length ?? 0) <= 50 ? load() : null), 30000, 120000)
usePoll(() => Promise.all([market.stocks ? null : market.refreshQuotes(), session.me ? account.refreshPositions() : null]), 60000)
const off = onLive('news', (m) => {
  if (!list.value || list.value.some((n) => n.id === m.item.id)) return
  const c = category(chip.value)
  if (c && m.item.category !== c) return
  list.value = [m.item, ...list.value]
})
onBeforeUnmount(off)

const held = computed(() => new Set((account.positions ?? []).map((p) => p.stock.id)))
const chips = computed(() => {
  const out = [{ key: 'all', label: '全部' }]
  if (session.me && held.value.size) out.push({ key: 'mine', label: '持仓相关' })
  for (const [k, v] of Object.entries(NEWS_CATEGORY)) out.push({ key: k, label: v.label })
  return out
})
const items = computed(() =>
  (list.value ?? []).filter((n) => chip.value !== 'mine' || (n.stock != null && held.value.has(n.stock.id))),
)
const days = computed(() => {
  const out: { day: string; items: NewsItem[] }[] = []
  for (const n of items.value) {
    const last = out[out.length - 1]
    if (last && last.day === n.trading_day) last.items.push(n)
    else out.push({ day: n.trading_day, items: [n] })
  }
  return out
})
const quote = (n: NewsItem) => (n.stock ? market.byId.get(n.stock.id) ?? null : null)
</script>

<template>
  <main class="page newspage">
    <header class="pagehead phone-only"><h1>资讯</h1></header>
    <div class="container">
      <h1 class="pagetitle desk-only">资讯</h1>
      <div class="chips gutter" role="group" aria-label="类型">
        <button v-for="c in chips" :key="c.key" type="button" :aria-pressed="chip === c.key" @click="chip = c.key">{{ c.label }}</button>
      </div>
      <EmptyState v-if="error && !list" :text="error" action="重试" tone="error" @act="refresh" />
      <section v-for="d in days" :key="d.day">
        <h2 class="dayh gutter">{{ dayLabel(d.day, session.status?.trading_day) }}</h2>
        <component :is="n.stock ? RouterLink : 'div'" v-for="n in d.items" :key="n.id" :to="n.stock ? `/stocks/${n.stock.id}` : undefined" class="item gutter" :class="{ fresh: market.fresh.has(n.id), hot: market.fresh.has(n.id) && n.category === 'flash' && n.stock != null && held.has(n.stock.id) }">
          <span class="num tm">{{ hm(n.at) }}</span>
          <span class="body">
            <span class="tags">
              <span class="kind" :class="newsGroup(n.category).tone">{{ newsGroup(n.category).label }}</span>
              <span v-if="n.stock && held.has(n.stock.id)" class="mine">持仓</span>
            </span>
            <span class="title">{{ n.title }}</span>
            <span v-if="n.body" class="text">{{ n.body }}</span>
            <span v-if="n.stock" class="stock">
              <Cover :id="n.stock.id" :title="n.stock.title" :w="14" :h="18" :spine="false" />
              <span class="clamp1">{{ n.stock.title }}</span>
              <span v-if="quote(n)" v-flash="quote(n)!.change" class="num" :class="dir(quote(n)!.change)">{{ pct(quote(n)!.change) }}</span>
            </span>
          </span>
        </component>
      </section>
      <EmptyState v-if="list && !items.length" text="暂无资讯" />
      <button v-if="list && next" type="button" class="linkbtn morebtn" :disabled="loading" @click="load(true)">{{ loading ? '加载中…' : '加载更多' }}</button>
      <div v-if="!list && !error" class="gutter"><div v-for="i in 4" :key="i" class="skel" style="height: 60px; margin: 14px 0"></div></div>
    </div>
  </main>
</template>

<style scoped>
.morebtn { display: block; margin: 16px auto; font-size: 13px; }
.chips { display: flex; gap: 8px; padding-top: 12px; padding-bottom: 12px; border-bottom: 1px solid var(--hair); overflow-x: auto; scrollbar-width: none; }
.chips button { flex: none; height: 30px; padding: 0 12px; font-size: 13px; border: 1px solid var(--base-line); background: transparent; color: var(--ink); }
.chips button[aria-pressed="true"] { background: var(--ink); color: var(--ground); border-color: var(--ink); }
.dayh { padding-top: 14px; padding-bottom: 6px; font-size: 13px; font-weight: 700; color: var(--ink-2); border-bottom: 1px solid var(--hair); }
.item { display: grid; grid-template-columns: 44px minmax(0, 1fr); column-gap: 10px; padding-top: 12px; padding-bottom: 12px; border-bottom: 1px solid var(--hair-2); color: var(--ink); }
/* an item that just arrived slides in; a 快讯 about a holding is highlighted a little longer */
.item.fresh { animation: news-in 0.35s ease-out, news-hl 1.6s ease-out; }
.item.hot { animation: news-in 0.35s ease-out, news-hot 3s ease-out; }
@keyframes news-in { from { transform: translateY(-8px); opacity: 0; } }
@keyframes news-hl { from { background-color: var(--flash-neutral); } }
@keyframes news-hot { 0%, 50% { background-color: color-mix(in srgb, var(--accent) 14%, transparent); } }
@media (prefers-reduced-motion: reduce) {
  .item.fresh { animation: news-hl 1.2s steps(1, end); }
  .item.hot { animation: news-hot 2.4s steps(1, end); }
}
.tm { font-size: 12px; color: var(--muted); padding-top: 3px; }
.body { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.tags { display: flex; align-items: center; gap: 8px; }
.kind { font-size: 11px; padding: 1px 6px; border: 1px solid currentColor; }
.kind.flash { color: var(--accent); }
.kind.notice { color: var(--ink); }
.kind.air { color: var(--mk-us); }
.kind.close { color: var(--ink-2); }
.mine { font-size: 11px; color: var(--accent); }
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
