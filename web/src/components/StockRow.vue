<script setup lang="ts">
import { RouterLink } from 'vue-router'
import type { MarketStock } from '../api'
import { money } from '../format'
import ChangePill from './ChangePill.vue'
import Cover from './Cover.vue'
defineProps<{ s: MarketStock; extra?: string }>()
</script>

<template>
  <RouterLink :to="`/stocks/${s.id}`" class="row">
    <Cover :id="s.id" :title="s.title" :w="36" :h="50" :spine="false" />
    <span class="name">
      <span class="serif clamp2 t">{{ s.title }}</span>
      <span class="num code">{{ s.code }}<template v-if="extra"> · {{ extra }}</template><span v-if="s.cooling" class="tag">冷静期</span><span v-else-if="s.new" class="tag">新上市</span></span>
    </span>
    <span class="q">
      <span v-flash="s.price" class="num px">{{ money(s.price) }}</span>
      <ChangePill :value="s.change" />
    </span>
  </RouterLink>
</template>

<style scoped>
.row { display: flex; align-items: center; gap: 12px; padding: 10px var(--gutter); border-bottom: 1px solid var(--hair-2); color: var(--ink); }
@media (hover: hover) { .row:hover { background: var(--ground-2); color: var(--ink); } }
.name { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.t { font-size: 15px; font-weight: 600; line-height: 1.3; }
.code { font-size: 11px; color: var(--muted); }
.tag { margin-left: 8px; font-family: var(--font); font-size: 11px; color: var(--ink-2); border: 1px solid currentColor; padding: 0 4px; }
.q { flex: none; display: flex; flex-direction: column; align-items: flex-end; gap: 4px; }
.px { font-size: 15px; font-weight: 500; }
</style>
