<script setup lang="ts">
// Search by title (Chinese / Japanese / English) or code, over the market list.
import { computed, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { money, pct, dir } from '../format'
import { useMarket } from '../stores/market'
import Icon from './Icon.vue'

const props = defineProps<{ compact?: boolean; autofocus?: boolean }>()
const emit = defineEmits<{ close: [] }>()
const market = useMarket()
const q = ref('')
const input = ref<HTMLInputElement>()
const open = ref(false)
onMounted(() => {
  if (props.autofocus) input.value?.focus()
  if (!market.stocks) market.refreshQuotes().catch(() => {})
})
const hits = computed(() => {
  const t = q.value.trim().toLowerCase()
  if (!t) return []
  return (market.stocks ?? [])
    .filter((s) => s.code.includes(t) || [s.title, s.title_ja, s.title_en].some((x) => (x ?? '').toLowerCase().includes(t)))
    .slice(0, 12)
})
function blur() {
  if (props.compact) window.setTimeout(() => (open.value = false), 150)
}
function done() {
  q.value = ''
  open.value = false
  emit('close')
}
</script>

<template>
  <div class="search" :class="{ compact }">
    <label class="box">
      <Icon name="search" :size="16" />
      <input ref="input" v-model="q" type="search" placeholder="搜索作品或代码" aria-label="搜索作品或代码" autocomplete="off"
             @focus="open = true" @blur="blur" @keydown.esc="done" />
    </label>
    <ul v-if="hits.length && (open || !compact)" class="hits" role="listbox">
      <li v-for="s in hits" :key="s.id">
        <RouterLink :to="`/stocks/${s.id}`" @click="done">
          <span class="clamp1 t">{{ s.title }}</span>
          <span class="num code">{{ s.code }}</span>
          <span class="num px" :class="dir(s.change)">{{ money(s.price) }} {{ pct(s.change) }}</span>
        </RouterLink>
      </li>
    </ul>
    <p v-else-if="q.trim() && (open || !compact)" class="none">没有匹配的作品</p>
  </div>
</template>

<style scoped>
.search { position: relative; }
.box { display: flex; align-items: center; gap: 8px; border: 1px solid var(--base-line); padding: 0 10px; height: 40px; background: var(--field-bg); color: var(--muted); }
.compact .box { height: 32px; }
input { border: 0; outline: 0; background: transparent; font-size: 16px; width: 100%; min-width: 0; color: var(--ink); }
.compact input { font-size: 13px; }
input::-webkit-search-cancel-button { display: none; }
.hits { margin-top: 8px; }
.compact .hits, .compact .none { position: absolute; left: 0; right: 0; top: 100%; margin-top: 4px; z-index: 40; background: var(--ground); border: 1px solid var(--rule); max-height: 60vh; overflow: auto; }
.hits a { display: grid; grid-template-columns: minmax(0, 1fr) auto; column-gap: 10px; padding: 10px 12px; border-bottom: 1px solid var(--hair-2); color: var(--ink); }
.t { font-size: 14px; }
.code { font-size: 11px; color: var(--muted); grid-row: 2; }
.px { grid-row: span 2; align-self: center; font-size: 13px; }
.none { padding: 14px 12px; font-size: 13px; color: var(--muted); }
</style>
