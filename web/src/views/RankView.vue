<script setup lang="ts">
import { computed, ref } from 'vue'
import { ApiError } from '../api'
import BackButton from '../components/BackButton.vue'
import EmptyState from '../components/EmptyState.vue'
import { usePoll } from '../composables/usePoll'
import { dir, money, pct } from '../format'
import { useSession } from '../stores/session'

const session = useSession()
const error = ref('')
const { refresh } = usePoll(async () => {
  try {
    await session.loadLeaders()
    error.value = ''
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '无法加载'
  }
}, 15000)
const rows = computed(() => session.leaders ?? [])
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
const mine = (name: string) => name === session.me?.username
</script>

<template>
  <main class="page rankpage">
    <header class="pagehead back phone-only">
      <BackButton />
      <h1>排行</h1>
    </header>
    <div class="container">
      <h1 class="pagetitle desk-only">排行</h1>
      <EmptyState v-if="error && !session.leaders" :text="error" action="重试" tone="error" @act="refresh" />
      <section v-if="podium.length" class="podium gutter">
        <div v-for="p in podium" :key="p.rank" class="pod">
          <span class="nm clamp1" :class="{ me: mine(p.username) }">{{ p.username }}</span>
          <span class="num ret" :class="dir(p.return_pct)">{{ pct(p.return_pct) }}</span>
          <span v-if="p.restructures" class="rs">重整 {{ p.restructures }} 次</span>
          <div class="block" :class="p.tone" :style="{ height: p.h + 'px' }"><span class="serif">{{ p.rank }}</span></div>
        </div>
      </section>
      <div v-if="rows.length" class="rrow head gutter"><span></span><span></span><span>总资产</span><span>收益率</span></div>
      <div v-for="r in rest" :key="r.username" class="rrow gutter" :class="{ me: mine(r.username) }">
        <span class="num rk">{{ r.rank }}</span>
        <span class="clamp1 nm">{{ r.username }}<span v-if="r.restructures" class="rs">重整 {{ r.restructures }} 次</span></span>
        <span class="num">{{ money(r.total) }}</span>
        <span class="num" :class="dir(r.return_pct)">{{ pct(r.return_pct) }}</span>
      </div>
      <EmptyState v-if="session.leaders && !rows.length" text="暂无玩家" action="注册" to="/login" />
    </div>
  </main>
</template>

<style scoped>
.podium { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); align-items: end; gap: 8px; padding-top: 22px; border-bottom: 1px solid var(--rule); }
.pod { display: flex; flex-direction: column; align-items: center; gap: 6px; min-width: 0; }
.pod .nm { font-size: 14px; font-weight: 700; max-width: 100%; }
.nm.me { color: var(--brand); }
.ret { font-size: 13px; }
.block { width: 100%; color: #fff; display: grid; place-items: start center; padding-top: 8px; font-size: 26px; font-weight: 900; }
.block.first { background: var(--brand); }
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
