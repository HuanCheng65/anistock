<script setup lang="ts">
import { computed } from 'vue'
import EmptyState from '../components/EmptyState.vue'
import StockRow from '../components/StockRow.vue'
import { usePoll } from '../composables/usePoll'
import { useMarket } from '../stores/market'
import { usePrefs } from '../stores/prefs'

const market = useMarket()
const prefs = usePrefs()
usePoll(() => market.refreshQuotes(), 5000)
const rows = computed(() => prefs.watchlist.map((id) => market.byId.get(id)).filter((s) => !!s))
</script>

<template>
  <main class="page watchpage">
    <header class="pagehead"><h1>自选</h1></header>
    <div class="wrap">
      <StockRow v-for="s in rows" :key="s!.id" :s="s!" />
      <EmptyState v-if="market.stocks && !rows.length" text="暂无自选" action="去看行情" to="/" />
      <EmptyState v-else-if="market.error && !market.stocks" :text="market.error" tone="error" />
    </div>
  </main>
</template>

<style scoped>
@media (min-width: 1024px) {
  .wrap, .pagehead { max-width: 760px; margin: 0 auto; }
  .pagehead { border-bottom: 1px solid var(--rule); }
}
</style>
