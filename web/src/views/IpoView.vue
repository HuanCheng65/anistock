<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { ApiError } from '../api'
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
    error.value = e instanceof ApiError ? e.message : '无法加载'
  }
}, 10000)
usePoll(() => (market.stocks ? null : market.refreshQuotes()), 60000)

const open = computed(() => (store.ipos ?? []).filter((i) => i.status === 'open'))
const mineDone = computed(() => (store.ipos ?? []).filter((i) => i.status !== 'open' && i.my))
const upcoming = computed(() => store.listings?.upcoming ?? [])
const recent = computed(() => store.listings?.recent ?? [])
const eps = (id: number) => market.byId.get(id)?.episodes ?? null
</script>

<template>
  <main class="page ipopage">
    <header class="pagehead phone-only">
      <h1>新股</h1>
      <span v-if="store.ipos" class="count">认购中 {{ open.length }}<template v-if="upcoming.length"> · 即将认购 {{ upcoming.length }}</template></span>
    </header>

    <div class="wrap">
      <p v-if="error" class="msg error gutter pad">{{ error }}</p>
      <div class="cols" :class="{ two: upcoming.length || recent.length }">
        <div class="col">
          <section v-for="i in open" :key="i.id" class="offer gutter">
            <IpoOffer :ipo="i" :episodes="eps(i.id)" @changed="refresh" />
          </section>
          <EmptyState v-if="store.ipos && !open.length" text="暂无认购中的新股" action="去看行情" to="/" />
          <div v-if="!store.ipos && !error" class="gutter pad"><div class="skel" style="height: 260px"></div></div>

          <section v-if="mineDone.length" class="sec gutter">
            <h2 class="h2">配售结果</h2>
            <div v-for="i in mineDone" :key="i.id" class="done">
              <RouterLink :to="`/stock/${i.id}`" class="serif clamp1 t">{{ i.title }}</RouterLink>
              <IpoOffer :ipo="i" compact />
            </div>
          </section>
        </div>

        <div class="col">
          <section v-if="upcoming.length" class="sec gutter">
            <h2 class="h2">即将认购</h2>
            <div v-for="u in upcoming" :key="u.id" class="soon">
              <Cover :id="u.id" :title="u.title" :w="30" :h="42" :spine="false" />
              <span class="serif clamp1 t">{{ u.title }}</span>
              <span class="num ink2 d">{{ u.sub_start.slice(5) }} 开始</span>
            </div>
          </section>

          <section v-if="recent.length" class="sec gutter">
            <h2 class="h2">近期上市</h2>
            <div class="lrow head"><span></span><span>发行价</span><span>首日</span><span>中签率</span></div>
            <RouterLink v-for="l in recent" :key="l.id" :to="`/stock/${l.id}`" class="lrow">
              <span class="serif clamp1 t">{{ l.title }}</span>
              <span class="num">{{ money(l.ipo_price) }}</span>
              <span class="num" :class="dir(l.first_change)">{{ l.first_change != null ? pct(l.first_change) : '–' }}</span>
              <span class="num">{{ l.ratio != null ? share(l.ratio, 1) : '–' }}</span>
            </RouterLink>
          </section>
        </div>
      </div>
    </div>
  </main>
</template>

<style scoped>
.pagehead { align-items: baseline; }
.count { font-size: 13px; color: var(--ink-2); }
.pad { padding-top: 16px; }
.offer { padding-top: 18px; padding-bottom: 18px; }
.offer + .offer { border-top: 1px solid var(--rule); }
.sec { border-top: 1px solid var(--rule); padding-top: 14px; padding-bottom: 6px; display: flex; flex-direction: column; margin-top: 18px; }
.sec .h2 { margin-bottom: 6px; }
.soon { display: flex; align-items: center; gap: 12px; padding: 10px 0; border-bottom: 1px solid var(--hair-2); }
.t { flex: 1; min-width: 0; font-size: 15px; font-weight: 600; color: var(--ink); }
.d { font-size: 13px; }
.lrow { display: grid; grid-template-columns: minmax(0, 1fr) 64px 70px 56px; gap: 8px; padding: 10px 0; border-bottom: 1px solid var(--hair-2); align-items: baseline; font-size: 13px; color: var(--ink); }
.lrow > span:not(:first-child) { text-align: right; }
.lrow .t { font-size: 14px; }
.lrow.head { padding: 6px 0; font-size: 12px; color: var(--muted); border-bottom-color: var(--hair); }
.done { display: flex; flex-direction: column; gap: 8px; padding: 10px 0; border-bottom: 1px solid var(--hair-2); }
@media (min-width: 1024px) {
  .wrap { max-width: 1200px; margin: 0 auto; }
  .cols.two { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 0; align-items: start; }
  .col { --gutter: 28px; min-width: 0; }
  .cols:not(.two) { max-width: 640px; margin: 0 auto; }
  .col + .col { border-left: 1px solid var(--hair); min-height: 60vh; }
  .col + .col .sec:first-child { margin-top: 0; border-top: 0; }
  .offer { padding-top: 24px; }
}
</style>
