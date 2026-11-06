<script setup lang="ts">
// 公告: the stock's news by time, newest first (flashes, premiere and finale, dividends, sequels, delays, splits,
// rights issues, buybacks, delisting warnings). 冷静期 and rate news are left out (stores/stock.ts NOTICE_KINDS).
import { ref } from 'vue'
import type { NewsItem } from '../../api'
import { shortDay } from '../../format'

const props = defineProps<{ items: NewsItem[] | null; more: boolean; today?: string; load: () => Promise<void>; title?: string }>()
/** the page already names the stock: a title starting with 《its name》 drops it (other works keep theirs) */
function short(t: string): string {
  const p = props.title ? `《${props.title}》` : ''
  return p && t.startsWith(p) && t.length > p.length ? t.slice(p.length) : t
}
const loading = ref(false)
const failed = ref(false)
async function loadMore() {
  if (loading.value) return
  loading.value = true
  failed.value = false
  try {
    await props.load()
  } catch {
    failed.value = true
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <div class="notices">
    <ol v-if="items && items.length" class="list">
      <li v-for="n in items" :key="n.id" class="ev">
        <span class="num d">{{ shortDay(n.trading_day, today) }}</span>
        <div class="tx">
          <p class="t">{{ short(n.title) }}</p>
          <p v-if="n.body" class="b">{{ n.body }}</p>
        </div>
      </li>
    </ol>
    <p v-else-if="items" class="none">暂无公告</p>
    <div v-else class="skel" style="height: 120px"></div>
    <p v-if="failed" class="msg error">无法加载，检查网络后重试</p>
    <button v-if="more" type="button" class="btn ghost small more" :disabled="loading" @click="loadMore">加载更多</button>
  </div>
</template>

<style scoped>
.notices { display: flex; flex-direction: column; gap: 12px; }
.ev { display: flex; gap: 14px; padding: 11px 0; border-bottom: 1px solid var(--hair-2); }
.ev:first-child { padding-top: 2px; }
.d { flex: none; width: 44px; font-size: 12px; color: var(--muted); padding-top: 2px; }
.tx { min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.t { font-size: 14px; line-height: 1.5; color: var(--ink); overflow-wrap: anywhere; }
.b { font-size: 13px; line-height: 1.55; color: var(--ink-2); overflow-wrap: anywhere; }
.none { font-size: 13px; color: var(--muted); padding: 8px 0; }
.more { align-self: center; }
@media (min-width: 1024px) {
  .d { width: 52px; }
  .tx { max-width: 72ch; }
}
</style>
