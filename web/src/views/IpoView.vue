<script setup lang="ts">
import { computed, ref } from 'vue'
import { api, ApiError, type Ipo } from '../api'
import IpoCard from '../components/IpoCard.vue'
import { usePoll } from '../composables/usePoll'

const ipos = ref<Ipo[] | null>(null)
const error = ref('')
const unavailable = ref(false)
const { refresh } = usePoll(async () => {
  try {
    ipos.value = (await api.get<{ ipos: Ipo[] }>('/api/ipos')).ipos
    error.value = ''
    unavailable.value = false
  } catch (e) {
    if (e instanceof ApiError && (e.status === 404 || e.status === 405)) {
      unavailable.value = true
      ipos.value = []
    } else error.value = e instanceof ApiError ? e.message : '加载失败'
  }
}, 10000)
const open = computed(() => (ipos.value ?? []).filter((i) => i.status === 'open'))
const done = computed(() => (ipos.value ?? []).filter((i) => i.status !== 'open'))
</script>

<template>
  <main class="page">
    <section class="intro">
      <h1>新股认购</h1>
      <p class="small muted">新番在首播前约 3 周上市。上市前 3 天开放认购，按发行价冻结资金，截止前可以改数量或撤回。认购截止当天日结时分配，第二天上市交易。</p>
    </section>
    <p v-if="error" class="notice error">{{ error }}</p>
    <p v-if="unavailable" class="notice">新股认购功能还在上线中，稍后再来看看。</p>

    <section class="panel" v-if="open.length">
      <div class="panel-head"><h2>认购中</h2><span class="small muted">{{ open.length }} 支</span></div>
      <div v-for="i in open" :key="i.id" class="item"><IpoCard :ipo="i" @changed="refresh" /></div>
    </section>
    <section class="panel" v-else-if="ipos && !unavailable">
      <p class="empty">现在没有正在认购的新股。新股开放认购时会出现在“新闻”里。</p>
    </section>
    <div v-else-if="!ipos" class="panel" style="padding: 16px"><div class="skeleton" style="height: 120px"></div></div>

    <section class="panel" v-if="done.length">
      <div class="panel-head"><h2>已分配</h2></div>
      <div v-for="i in done" :key="i.id" class="item"><IpoCard :ipo="i" /></div>
    </section>
  </main>
</template>

<style scoped>
.intro { display: flex; flex-direction: column; gap: 4px; padding: 4px 2px; }
.intro h1 { font-size: 20px; font-weight: 700; }
.item { padding: 12px; border-bottom: 1px solid var(--rule); }
.item:last-child { border-bottom: none; }
</style>
