<script setup lang="ts">
import { ref } from 'vue'
import { api, ApiError, type LeaderRow } from '../api'
import { usePoll } from '../composables/usePoll'
import { dir, money, pct } from '../format'
import { useSession } from '../stores/session'

const session = useSession()
const rows = ref<LeaderRow[] | null>(null)
const error = ref('')
usePoll(async () => {
  try {
    rows.value = (await api.get<{ players: LeaderRow[] }>('/api/leaderboard')).players
    error.value = ''
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '加载失败'
  }
}, 15000)
</script>

<template>
  <main class="page">
    <section class="panel">
      <div class="panel-head"><h2>总资产排行</h2><span class="small muted">现金 + 持仓按最新价计</span></div>
      <p v-if="error" class="notice error" style="margin: 12px">{{ error }}</p>
      <ol v-if="rows && rows.length" class="board">
        <li v-for="r in rows" :key="r.username" :class="{ me: r.username === session.me?.username }">
          <span class="rank num" :class="{ podium: r.rank <= 3 }">{{ r.rank }}</span>
          <span class="name">{{ r.username }}<span v-if="r.username === session.me?.username" class="small muted">（我）</span></span>
          <span class="num total">{{ money(r.total) }}</span>
          <span class="num ret" :class="dir(r.return_pct)">{{ pct(r.return_pct) }}</span>
        </li>
      </ol>
      <p v-else-if="rows" class="empty">还没有玩家，来当第一个吧</p>
      <div v-else class="panel-body"><div class="skeleton" style="height: 120px"></div></div>
    </section>
  </main>
</template>

<style scoped>
.board { list-style: none; margin: 0; padding: 0; }
.board li { display: grid; grid-template-columns: 2.4em minmax(0, 1fr) auto 5.5em; gap: 10px; align-items: center; padding: 11px 12px; border-bottom: 1px solid var(--rule); }
.board li:last-child { border-bottom: none; }
.board li.me { background: var(--surface-2); }
.rank { color: var(--muted); font-weight: 600; text-align: center; }
.rank.podium { color: #fff; background: var(--seal); border-radius: 3px; padding: 1px 0; }
.name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
.total { font-weight: 600; }
.ret { text-align: right; }
</style>
