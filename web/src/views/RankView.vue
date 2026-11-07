<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { errorText } from '../api'
import BackButton from '../components/BackButton.vue'
import EmptyState from '../components/EmptyState.vue'
import Tabs from '../components/Tabs.vue'
import { usePoll } from '../composables/usePoll'
import { dir, money, pct, shortDay } from '../format'
import { useSession, type Period } from '../stores/session'

const session = useSession()
const error = ref('')
const period = ref<Period>('all')
const periods = [
  { key: 'all', label: '总榜' },
  { key: 'week', label: '周榜' },
  { key: 'season', label: '季榜' },
]
const periodModel = computed({ get: () => period.value as string, set: (v: string) => (period.value = v as Period) })
const { refresh } = usePoll(async () => {
  try {
    await session.loadLeaders(period.value)
    error.value = ''
  } catch (e) {
    error.value = errorText(e, '无法加载')
  }
}, 15000)
watch(period, () => refresh())
const board = computed(() => session.boards[period.value] ?? null)
const rows = computed(() => board.value?.items ?? [])
const since = computed(() => (board.value?.from_day && period.value !== 'all' ? `${shortDay(board.value.from_day, session.status?.trading_day)} 起` : ''))
const podium = computed(() => {
  const r = rows.value
  if (r.length < 3) return []
  return [
    { ...r[1], h: 72, tone: 'second' },
    { ...r[0], h: 104, tone: 'first' },
    { ...r[2], h: 52, tone: 'third' },
  ]
})
const rest = computed(() => (podium.value.length ? rows.value.slice(3) : rows.value))
const mine = (id: number) => id === session.me?.id
const unranked = computed(() => (session.me && !session.me.eligibility.leaderboard.eligible ? session.me.eligibility.leaderboard.reason : null))
</script>

<template>
  <main class="page rankpage">
    <header class="pagehead back phone-only">
      <BackButton />
      <h1>排行</h1>
    </header>
    <div class="container">
      <h1 class="pagetitle desk-only">排行</h1>
      <Tabs v-model="periodModel" :items="periods" label="排行周期" class="ptabs gutter"><span v-if="since" class="since num">{{ since }}</span></Tabs>
      <EmptyState v-if="error && !board" :text="error" action="重试" tone="error" @act="refresh" />
      <p v-if="unranked" class="unranked gutter">{{ unranked }}</p>
      <section v-if="podium.length" class="podium gutter">
        <div v-for="p in podium" :key="p.rank" class="pod">
          <span class="nm clamp1" :class="{ me: mine(p.user.id) }">{{ p.user.username }}</span>
          <span class="num ret" :class="dir(p.return)">{{ pct(p.return) }}</span>
          <span v-if="p.restructures" class="rs">重整 {{ p.restructures }} 次</span>
          <span v-if="p.program" class="rs">程序化</span>
          <div class="block" :class="p.tone" :style="{ height: p.h + 'px' }"><span class="serif">{{ p.rank }}</span></div>
        </div>
      </section>
      <div v-if="rows.length" class="rrow head gutter"><span></span><span></span><span>总资产</span><span>收益率</span></div>
      <div v-for="r in rest" :key="r.user.id" class="rrow gutter" :class="{ me: mine(r.user.id) }">
        <span class="num rk">{{ r.rank }}</span>
        <span class="clamp1 nm">{{ r.user.username }}<span v-if="r.program" class="rs">程序化</span><span v-if="r.restructures" class="rs">重整 {{ r.restructures }} 次</span></span>
        <span class="num">{{ money(r.total_assets) }}</span>
        <span class="num" :class="dir(r.return)">{{ pct(r.return) }}</span>
      </div>
      <template v-if="board && !rows.length">
        <EmptyState v-if="session.me" text="暂无上榜玩家" />
        <EmptyState v-else text="暂无上榜玩家" action="注册" to="/login" />
      </template>
    </div>
  </main>
</template>

<style scoped>
.unranked { padding-top: 10px; padding-bottom: 10px; font-size: 13px; color: var(--ink-2); border-bottom: 1px solid var(--hair); }
.ptabs { gap: 20px; }
.since { margin-left: auto; font-size: 12px; color: var(--muted); align-self: center; }
.podium { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); align-items: end; gap: 8px; padding-top: 22px; border-bottom: 1px solid var(--rule); }
.pod { display: flex; flex-direction: column; align-items: center; gap: 6px; min-width: 0; }
.pod .nm { font-size: 14px; font-weight: 700; max-width: 100%; }
.nm.me { color: var(--accent); }
.ret { font-size: 13px; }
.block { width: 100%; color: #fff; display: grid; place-items: start center; padding-top: 8px; font-size: 26px; font-weight: 900; }
.block.first { background: var(--ink); }
.block.second { background: #4B4F58; }
.block.third { background: #8A8E96; }
.rrow { display: grid; grid-template-columns: 36px minmax(0, 1fr) 110px 76px; gap: 8px; padding-top: 12px; padding-bottom: 12px; border-bottom: 1px solid var(--hair-2); align-items: baseline; font-size: 14px; }
.rrow > :nth-child(n + 3) { text-align: right; }
.rrow.head { padding-top: 10px; padding-bottom: 6px; font-size: 12px; color: var(--muted); border-bottom-color: var(--hair); }
.rrow.me { background: var(--ground-2); font-weight: 700; }
.rk { color: var(--ink-2); }
.rs { font-size: 12px; font-weight: 400; color: var(--muted); }
.nm .rs { margin-left: 8px; }
.nm { font-size: 15px; }
@media (min-width: 1024px) {
  .podium { max-width: 520px; padding-top: 0; border-bottom: 0; margin-bottom: 24px; }
  .rrow { grid-template-columns: 60px minmax(0, 1fr) 200px 140px; }
  .rrow.head { border-top: 1px solid var(--rule); }
}
</style>
