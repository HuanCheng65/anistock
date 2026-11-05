<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { api, ApiError, type NewsItem } from '../api'
import { usePoll } from '../composables/usePoll'
import { NEWS_KIND } from '../format'

const items = ref<NewsItem[] | null>(null)
const error = ref('')
const kind = ref('all')
usePoll(async () => {
  try {
    items.value = (await api.get<{ news: NewsItem[] }>('/api/news?limit=150')).news
    error.value = ''
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '加载失败'
  }
}, 30000)
const shown = computed(() => (items.value ?? []).filter((n) => kind.value === 'all' || n.kind === kind.value))
const kinds = ['all', 'ipo', 'allotment', 'premiere', 'finale', 'dividend', 'vcm', 'rate', 'delisting', 'system']
</script>

<template>
  <main class="page">
    <div class="chips" role="group" aria-label="新闻类型">
      <button v-for="k in kinds" :key="k" type="button" class="chip" :aria-pressed="kind === k" @click="kind = k">{{ k === 'all' ? '全部' : NEWS_KIND[k] }}</button>
    </div>
    <p v-if="error" class="notice error">{{ error }}</p>
    <section class="panel">
      <ul v-if="shown.length" class="feed">
        <li v-for="n in shown" :key="n.id">
          <div class="when small">
            <span class="kind" :class="n.kind">{{ NEWS_KIND[n.kind] ?? n.kind }}</span>
            <span class="muted num">{{ n.day }}</span>
            <RouterLink v-if="n.company_id" :to="`/stock/${n.company_id}`" class="num">{{ n.code }}</RouterLink>
          </div>
          <b>{{ n.title }}</b>
          <p v-if="n.body" class="small muted">{{ n.body }}</p>
        </li>
      </ul>
      <p v-else-if="items" class="empty">暂时没有这类新闻</p>
      <div v-else class="panel-body"><div class="skeleton" style="height: 140px"></div></div>
    </section>
  </main>
</template>

<style scoped>
.chips { display: flex; gap: 6px; overflow-x: auto; scrollbar-width: none; }
.chip { flex: none; height: 30px; padding: 0 12px; border-radius: 15px; border: 1px solid var(--rule-strong); background: var(--surface); font-size: 13px; cursor: pointer; color: var(--ink-2); }
.chip[aria-pressed="true"] { background: var(--ink); color: var(--surface); border-color: var(--ink); }
.feed { list-style: none; margin: 0; padding: 0; }
.feed li { display: flex; flex-direction: column; gap: 3px; padding: 12px; border-bottom: 1px solid var(--rule); }
.feed li:last-child { border-bottom: none; }
.when { display: flex; gap: 8px; align-items: center; }
.kind { font-size: 11px; padding: 1px 6px; border-radius: 3px; background: var(--surface-2); color: var(--ink-2); font-weight: 600; }
.kind.ipo, .kind.allotment { background: var(--seal); color: #fff; }
.kind.vcm { background: var(--warn-bg); color: var(--warn); }
.kind.rate { background: var(--ink); color: var(--surface); }
.kind.dividend { color: var(--up); background: var(--up-bg); }
.kind.premiere { color: var(--accent); }
.kind.delisting { color: var(--down); background: var(--down-bg); }
</style>
