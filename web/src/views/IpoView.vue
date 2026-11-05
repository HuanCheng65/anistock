<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { errorText } from '../api'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import IpoOffer from '../components/IpoOffer.vue'
import { usePoll } from '../composables/usePoll'
import { dir, money, pct, share } from '../format'
import { useIpo } from '../stores/ipo'
import { useMarket } from '../stores/market'

const store = useIpo()
const market = useMarket()
const error = ref('')
const { refresh } = usePoll(async () => {
  try {
    await store.refresh()
    error.value = ''
  } catch (e) {
    error.value = errorText(e, '无法加载')
  }
}, 10000)
usePoll(() => (market.stocks ? null : market.refreshQuotes()), 60000)

const open = computed(() => store.open)
const mineDone = computed(() => (store.ipos ?? []).filter((i) => (i.status === 'allotted' || i.status === 'listed') && i.viewer?.quantity))
const upcoming = computed(() => store.upcoming)
const recent = computed(() => store.listed.slice(0, 20))
const eps = (id: number) => market.byId.get(id)?.episodes ?? null
</script>

<template>
  <main class="page ipopage">
    <header class="pagehead phone-only">
      <h1>新股</h1>
      <span v-if="store.ipos" class="count">认购中 {{ open.length }}<template v-if="upcoming.length"> · 即将认购 {{ upcoming.length }}</template></span>
    </header>

    <div class="container">
      <h1 class="pagetitle desk-only">新股</h1>
      <p v-if="error" class="msg error pad">{{ error }}</p>
      <section v-for="i in open" :key="i.stock.id" class="offerwrap">
        <IpoOffer :ipo="i" :episodes="eps(i.stock.id)" @changed="refresh" />
      </section>
      <EmptyState v-if="store.ipos && !open.length" text="暂无认购中的新股" action="去看股票" to="/stocks" />
      <div v-if="!store.ipos && !error" class="pad"><div class="skel" style="height: 220px"></div></div>

      <div class="cols">
        <section class="sec">
          <h2 class="h2">即将认购</h2>
          <div v-for="u in upcoming" :key="u.stock.id" class="soon">
            <Cover :id="u.stock.id" :title="u.stock.title" :w="24" :h="34" :spine="false" />
            <span class="serif clamp1 t">《{{ u.stock.title }}》</span>
            <span class="num ink2 d">{{ u.sub_start.slice(5) }} 开始</span>
            <span class="num ink2 d">{{ u.first_air.slice(5) }} 首播</span>
          </div>
          <p v-if="!upcoming.length" class="none">暂无</p>
        </section>

        <section class="sec">
          <h2 class="h2">近期上市</h2>
          <template v-if="recent.length">
            <div class="lrow head"><span></span><span>发行价</span><span>首日</span><span>中签率</span><span class="desk-only">认购倍数</span></div>
            <RouterLink v-for="l in recent" :key="l.stock.id" :to="`/stocks/${l.stock.id}`" class="lrow">
              <span class="serif clamp1 t">{{ l.stock.title }}</span>
              <span class="num">{{ money(l.price) }}</span>
              <span class="num" :class="dir(l.first_day?.change)">{{ l.first_day?.change != null ? pct(l.first_day.change) : '–' }}</span>
              <span class="num">{{ l.result?.allotment_ratio != null ? share(l.result.allotment_ratio, 1) : '–' }}</span>
              <span class="num desk-only">{{ l.result ? l.result.oversubscribed.toFixed(1) + '×' : '–' }}</span>
            </RouterLink>
          </template>
          <p v-else class="none">暂无</p>
        </section>
      </div>

      <section v-if="mineDone.length" class="sec">
        <h2 class="h2">配售结果</h2>
        <div v-for="i in mineDone" :key="i.stock.id" class="done">
          <RouterLink :to="`/stocks/${i.stock.id}`" class="serif clamp1 t">{{ i.stock.title }}</RouterLink>
          <IpoOffer :ipo="i" compact />
        </div>
      </section>
    </div>
  </main>
</template>

<style scoped>
.pagehead { align-items: baseline; }
.count { font-size: 13px; color: var(--ink-2); }
.pad { padding-top: 16px; }
.offerwrap { padding-top: 18px; padding-bottom: 22px; }
.offerwrap + .offerwrap { border-top: 1px solid var(--hair); }
.sec { padding-top: 14px; padding-bottom: 6px; display: flex; flex-direction: column; margin-top: 18px; }
.sec .h2 { padding-bottom: 8px; border-bottom: 1px solid var(--rule); }
.none { padding: 14px 0; font-size: 13px; color: var(--muted); }
.soon { display: grid; grid-template-columns: 24px minmax(0, 1fr) auto auto; gap: 12px; align-items: center; padding: 10px 0; border-bottom: 1px solid var(--hair-2); }
.t { min-width: 0; font-size: 15px; font-weight: 600; color: var(--ink); }
.d { font-size: 13px; }
.lrow { display: grid; grid-template-columns: minmax(0, 1fr) 64px 70px 56px; gap: 8px; padding: 10px 0; border-bottom: 1px solid var(--hair-2); align-items: baseline; font-size: 13px; color: var(--ink); }
.lrow > span:not(:first-child) { text-align: right; }
.lrow .t { font-size: 14px; }
.lrow.head { padding: 6px 0; font-size: 12px; color: var(--muted); border-bottom-color: var(--hair); }
.done { display: flex; flex-direction: column; gap: 8px; padding: 10px 0; border-bottom: 1px solid var(--hair-2); }
@media (min-width: 1024px) {
  .offerwrap { padding-top: 0; padding-bottom: 32px; border-bottom: 1px solid var(--rule); }
  .cols { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); column-gap: 40px; align-items: start; }
  .sec { margin-top: 24px; }
  .lrow { grid-template-columns: minmax(0, 1fr) 80px 80px 70px 70px; }
}
</style>
