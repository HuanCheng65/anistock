<script setup lang="ts">
// 行情's kinds: 股票 / 热度期货 / 基金, each shown once it is open (a kind with nothing listed is left out; with only
// stocks, no switch at all).
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { usePoll } from '../composables/usePoll'
import { useFunds } from '../stores/funds'
import { useFutures } from '../stores/futures'

const props = defineProps<{ current: 'stocks' | 'futures' | 'funds' }>()
const fut = useFutures()
const funds = useFunds()
usePoll(() => (fut.list ? null : fut.refreshList().catch(() => {})), 300000)
usePoll(() => (funds.list ? null : funds.refreshList().catch(() => {})), 300000)
const kinds = computed(() =>
  [
    { key: 'stocks', label: '股票', to: '/stocks', on: true },
    { key: 'futures', label: '热度期货', to: '/futures', on: !!fut.list?.length || props.current === 'futures' },
    { key: 'funds', label: '基金', to: '/funds', on: !!funds.list?.length || props.current === 'funds' },
  ].filter((k) => k.on),
)
</script>

<template>
  <nav v-if="kinds.length > 1" class="seg" aria-label="品种" :style="{ gridTemplateColumns: `repeat(${kinds.length}, minmax(0, 1fr))` }">
    <template v-for="k in kinds" :key="k.key">
      <span v-if="k.key === current" aria-current="page">{{ k.label }}</span>
      <RouterLink v-else :to="k.to">{{ k.label }}</RouterLink>
    </template>
  </nav>
</template>

<style scoped>
.seg { display: grid; border: 1px solid var(--rule); margin-top: 4px; max-width: 360px; }
.seg a, .seg span { height: 36px; display: flex; align-items: center; justify-content: center; font-size: 14px; color: var(--ink-2); }
.seg [aria-current] { background: var(--ink); color: var(--ground); font-weight: 700; }
</style>
