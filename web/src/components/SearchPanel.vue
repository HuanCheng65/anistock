<script setup lang="ts">
// Search by any name of the work (Chinese, Japanese, romaji, English, aliases) or its code
// (/api/v1/stocks/search: the server ranks the matches).
import { onMounted, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { api, call, type SearchHit } from '../api'
import { money, pct, dir } from '../format'
import Icon from './Icon.vue'

const props = defineProps<{ compact?: boolean; autofocus?: boolean }>()
const emit = defineEmits<{ close: [] }>()
const q = ref('')
const input = ref<HTMLInputElement>()
const open = ref(false)
const hits = ref<SearchHit[]>([])
const searched = ref('')
onMounted(() => {
  if (props.autofocus) input.value?.focus()
})
let timer: number | undefined
let seq = 0
watch(q, (v) => {
  window.clearTimeout(timer)
  const t = v.trim()
  if (!t) {
    hits.value = []
    searched.value = ''
    return
  }
  timer = window.setTimeout(async () => {
    const n = ++seq
    try {
      const r = await call(api.GET('/api/v1/stocks/search', { params: { query: { q: t.slice(0, 50), limit: 12 } } }))
      if (n === seq) {
        hits.value = r.items
        searched.value = t
      }
    } catch {
      /* keep the last results */
    }
  }, 160)
})
/** the name that matched, when it is not the title shown */
const alias = (h: SearchHit) => (h.matched.field === 'title' || h.matched.field === 'code' ? '' : h.matched.text)
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
      <li v-for="h in hits" :key="h.stock.id">
        <RouterLink :to="`/stocks/${h.stock.id}`" @click="done">
          <span class="clamp1 t">{{ h.stock.title }}</span>
          <span class="num code">{{ h.stock.code }}<template v-if="alias(h)"> · <span class="sans">{{ alias(h) }}</span></template><template v-if="h.status === 'subscribing'"> · <span class="sans">认购中</span></template></span>
          <span class="num px" :class="dir(h.change)">{{ money(h.price) }} {{ pct(h.change) }}</span>
        </RouterLink>
      </li>
    </ul>
    <p v-else-if="searched && q.trim() && (open || !compact)" class="none">没有匹配的作品</p>
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
.code { font-size: 11px; color: var(--muted); grid-row: 2; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sans { font-family: var(--font); }
.px { grid-row: span 2; align-self: center; font-size: 13px; }
.none { padding: 14px 12px; font-size: 13px; color: var(--muted); }
</style>
