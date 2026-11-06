<script setup lang="ts">
// 新手引导 after the first sign-in on this device: pick a work, how revenue and dividends work, first buy.
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { api, call, errorText } from '../api'
import Cover from '../components/Cover.vue'
import { usePoll } from '../composables/usePoll'
import { dir, money, pct, qty, taxBrackets } from '../format'
import { useMarket } from '../stores/market'
import { usePrefs } from '../stores/prefs'
import { useSession } from '../stores/session'

const market = useMarket()
const prefs = usePrefs()
const session = useSession()
const router = useRouter()
const route = useRoute()
const step = ref(0)
const pickId = ref<number | null>(null)
const busy = ref(false)
const error = ref('')
usePoll(() => market.refreshQuotes(), 15000)

// six works on air right now, by this week's revenue
const picks = computed(() =>
  [...(market.stocks ?? [])]
    .filter((s) => !s.is_long_runner && s.episodes_aired > 0 && (!s.episodes || s.episodes_aired < s.episodes))
    .sort((a, b) => b.week_revenue - a.week_revenue)
    .slice(0, 6),
)
const chosen = computed(() => market.byId.get(pickId.value ?? -1) ?? picks.value[0] ?? null)
// about 10,000 星辉 (cents)
const buyQty = computed(() => (chosen.value && chosen.value.price > 0 ? Math.floor(1_000_000 / chosen.value.price) : 0))
session.loadRules()
const tax = computed(() => taxBrackets(session.rules?.dividend_tax))
const trading = computed(() => session.status?.phase === 'continuous')
const note = computed(() => {
  if (step.value === 0) return chosen.value ? `《${chosen.value.stock.title}》 现价 ${money(chosen.value.price)}` : ''
  if (step.value === 2) return chosen.value ? (trading.value ? `先用 1万星辉试试：约 ${qty(buyQty.value)} 股` : '休市中，开盘后可以买入') : ''
  return ''
})
const next = () => (step.value = Math.min(2, step.value + 1))
function finish(to?: string) {
  prefs.onboarded = true
  router.replace(to ?? (typeof route.query.next === 'string' ? route.query.next : '/'))
}
async function buy() {
  const s = chosen.value
  if (!s) return finish()
  if (!trading.value || !buyQty.value) return finish(`/stocks/${s.stock.id}`)
  busy.value = true
  error.value = ''
  try {
    await call(api.POST('/api/v1/me/orders', { body: { stock_id: s.stock.id, side: 'buy', type: 'market', quantity: buyQty.value } }))
    finish(`/stocks/${s.stock.id}`)
  } catch (e) {
    error.value = errorText(e, '无法提交委托')
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <main class="welcome">
    <div class="top">
      <span v-for="i in 3" :key="i" class="dot" :class="{ on: i - 1 <= step }"></span>
      <button type="button" class="skip" @click="step < 2 ? (step = 2) : finish()">跳过</button>
    </div>

    <template v-if="step === 0">
      <div class="head">
        <div class="num n">1 / 3</div>
        <h1 class="serif">每部番都是一支股票</h1>
        <p>选一部你在追的。</p>
      </div>
      <div class="grid">
        <button v-for="s in picks" :key="s.stock.id" type="button" class="pick" :aria-pressed="chosen?.stock.id === s.stock.id" @click="pickId = s.stock.id">
          <Cover :id="s.stock.id" :title="s.stock.title" :w="110" :h="138" :size="16" class="pc" />
          <span class="num" :class="dir(s.change)">{{ pct(s.change) }}</span>
        </button>
      </div>
    </template>

    <template v-else-if="step === 1">
      <div class="head">
        <div class="num n">2 / 3</div>
        <h1 class="serif">番越火，公司越赚钱</h1>
        <p>营收来自真实观众的讨论和观看，每天结算一次。股价跟着大家对营收的预期涨跌。</p>
      </div>
      <svg viewBox="0 0 350 180" class="demo" role="img" aria-label="热度上升时股价随之上涨">
        <line x1="0" y1="160" x2="350" y2="160" class="b" />
        <path d="M0 140 L50 132 L100 120 L150 104 L200 96 L250 70 L300 56 L350 40" class="heat" />
        <path d="M0 150 L50 146 L100 128 L150 118 L200 110 L250 80 L300 72 L350 50" class="px" />
        <line x1="250" y1="20" x2="250" y2="160" class="ep" />
        <rect x="226" y="164" width="48" height="16" class="tag" />
        <text x="250" y="176" text-anchor="middle">第 5 集</text>
      </svg>
      <div class="legend">
        <span><i class="lp"></i>股价</span>
        <span><i class="lh"></i>热度</span>
      </div>
    </template>

    <template v-else>
      <div class="head">
        <div class="num n">3 / 3</div>
        <h1 class="serif">拿住，每周日分红</h1>
        <p>公司把利润按持股分给股东。持有越久，分红的税越低。</p>
      </div>
      <div class="tax">
        <div v-for="t in tax" :key="t.k" class="tr"><span>{{ t.k }}</span><span class="num">{{ t.v }}</span></div>
      </div>
    </template>

    <div class="bottom">
      <div class="note">{{ note }}</div>
      <p v-if="error" class="msg error">{{ error }}</p>
      <button v-if="step < 2" type="button" class="btn block cta" @click="next">下一步</button>
      <button v-else-if="trading && chosen" type="button" class="btn block cta buy" :disabled="busy" @click="buy">买入 {{ qty(buyQty) }} 股</button>
      <button v-else type="button" class="btn block cta" @click="finish(chosen ? `/stocks/${chosen.stock.id}` : '/stocks')">去看股票</button>
    </div>
  </main>
</template>

<style scoped>
.welcome { min-height: 100vh; min-height: 100dvh; max-width: 480px; margin: 0 auto; padding: 0 20px; display: flex; flex-direction: column; }
.top { display: flex; align-items: center; gap: 6px; padding-top: calc(22px + env(safe-area-inset-top, 0px)); }
.dot { flex: 1; height: 3px; background: var(--hair); }
.dot.on { background: var(--ink); }
.skip { margin-left: 10px; border: 0; background: transparent; font-size: 13px; color: var(--ink-2); padding: 6px 0; }
.head { display: flex; flex-direction: column; gap: 10px; padding: 30px 0 18px; }
.n { font-size: 13px; color: var(--accent); }
.head h1 { font-size: 30px; font-weight: 900; line-height: 1.25; }
.head p { font-size: 15px; line-height: 1.6; color: var(--ink-2); }
.grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; }
.pick { border: 0; padding: 0; background: transparent; display: flex; flex-direction: column; gap: 6px; text-align: left; font-size: 12px; }
.pc { width: 100% !important; outline: 2px solid transparent; outline-offset: 2px; }
.pick[aria-pressed="true"] .pc { outline-color: var(--ink); }
.demo { width: 100%; height: auto; }
.demo .b { stroke: var(--base-line); }
.demo .heat { fill: none; stroke: var(--hold-npc); stroke-width: 2; stroke-dasharray: 4 4; }
.demo .px { fill: none; stroke: var(--up); stroke-width: 2.4; stroke-linejoin: round; }
.demo .ep { stroke: var(--accent); stroke-dasharray: 2 3; }
.demo .tag { fill: var(--accent); }
.demo text { font-size: 11px; fill: #fff; font-family: var(--font); }
.legend { display: flex; gap: 16px; font-size: 12px; color: var(--ink-2); padding-top: 6px; }
.legend span { display: flex; align-items: center; gap: 6px; }
.lp { width: 14px; border-top: 2px solid var(--up); }
.lh { width: 14px; border-top: 2px dashed var(--hold-npc); }
.tax { border-top: 1px solid var(--rule); }
.tr { display: flex; justify-content: space-between; padding: 12px 0; border-bottom: 1px solid var(--hair); font-size: 15px; }
.bottom { margin-top: auto; padding: 24px 0 calc(28px + env(safe-area-inset-bottom, 0px)); display: flex; flex-direction: column; gap: 10px; }
.note { font-size: 13px; color: var(--ink-2); min-height: 20px; }
.cta { height: 50px; font-size: 16px; }
.cta.buy { background: var(--up); color: #fff; }
</style>
