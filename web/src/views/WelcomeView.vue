<script setup lang="ts">
// Account opening (docs/design/account.md §7). Right after registering (?new=1): what AniEx is (with three
// stocks trading now), the risk disclosure, the receipt. After a sign-in with documents pending (existing players, a new version):
// accept the updated documents, sign the disclosure. Nothing here places an order.
import { computed, nextTick, onMounted, ref } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { api, call, errorText } from '../api'
import Cover from '../components/Cover.vue'
import Icon from '../components/Icon.vue'
import LegalText from '../components/LegalText.vue'
import { dir, money, pct, when } from '../format'
import { DOCUMENTS, isDocKey } from '../legal/documents'
import { useMarket } from '../stores/market'
import { useSession } from '../stores/session'

type Step = 'intro' | 'accept' | 'sign' | 'done'

const session = useSession()
const router = useRouter()
const route = useRoute()
const next = typeof route.query.next === 'string' ? route.query.next : '/'
const isNew = route.query.new === '1'

// fixed when the page opens, so a step does not vanish once its document is signed
const pending = session.me?.pending_documents ?? []
const toAccept = pending.filter((d) => d.doc !== 'disclosure' && isDocKey(d.doc))
const toSign = pending.find((d) => d.doc === 'disclosure')
const steps: Step[] = [
  ...(isNew ? (['intro'] as const) : []),
  ...(toAccept.length ? (['accept'] as const) : []),
  ...(toSign ? (['sign'] as const) : []),
  ...(isNew ? (['done'] as const) : []),
]
if (!steps.length) router.replace(next)

// the intro shows the three stocks with the most turnover today: real works being traded, not a diagram
const market = useMarket()
const busiest = computed(() => [...(market.stocks ?? [])].sort((a, b) => b.turnover - a.turnover).slice(0, 3))
onMounted(() => {
  if (isNew && !market.stocks) market.refreshQuotes().catch(() => {})
})

const at = ref(0)
const step = computed(() => steps[at.value])
const busy = ref(false)
const error = ref('')
const signature = ref('')
const signed = computed(() => signature.value.trim() === session.me?.username)
const body = ref<HTMLElement | null>(null)

async function advance() {
  error.value = ''
  if (at.value < steps.length - 1) {
    at.value += 1
    await nextTick()
    body.value?.scrollTo?.({ top: 0 })
  } else finish()
}
function finish(to = next) {
  router.replace(to)
}

async function post(doc: string, version: number, sig?: string) {
  session.me = await call(api.POST('/api/v1/me/agreements', { body: { doc, version, signature: sig ?? null } }))
}
async function accept() {
  busy.value = true
  try {
    for (const d of toAccept) await post(d.doc, d.version)
    await advance()
  } catch (e) {
    error.value = errorText(e)
  } finally {
    busy.value = false
  }
}
async function sign() {
  if (!toSign || !signed.value) return
  busy.value = true
  try {
    await post(toSign.doc, toSign.version, signature.value.trim())
    await advance()
  } catch (e) {
    error.value = errorText(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <main v-if="step" class="welcome">
    <div class="top">
      <template v-if="steps.length > 1">
        <span v-for="(s, i) in steps" :key="s" class="dot" :class="{ on: i <= at }"></span>
      </template>
      <button v-if="step !== 'done'" type="button" class="skip" @click="finish()">稍后</button>
    </div>

    <Transition name="step" mode="out-in">
      <section :key="step" ref="body" class="body">
        <template v-if="step === 'intro'">
          <h1 class="serif">动画作品交易所</h1>
          <p class="lede">安易是动画作品的模拟交易所。每部在播或即将开播的作品都有一支股票，价格由玩家之间的买卖决定。看好一部作品就买入，不看好就卖出，和其他玩家比较收益。</p>
          <p class="lede">熟悉以后，还可以认购新股、交易热度期货、申购指数基金。</p>
          <ul v-if="busiest.length" class="live">
            <li v-for="x in busiest" :key="x.stock.id">
              <Cover :id="x.stock.id" :title="x.stock.title" :w="30" :h="42" :spine="false" />
              <span class="serif clamp1 t">{{ x.stock.title }}</span>
              <span class="num p">{{ money(x.price) }}</span>
              <span class="num c" :class="dir(x.change)">{{ pct(x.change) }}</span>
            </li>
          </ul>
        </template>

        <template v-else-if="step === 'accept'">
          <h1 class="serif">协议更新</h1>
          <p class="lede">继续交易前，需要同意更新后的文件。</p>
          <nav class="rows">
            <RouterLink v-for="d in toAccept" :key="d.doc" :to="`/legal/${d.doc}`" class="row">
              <span>{{ DOCUMENTS[d.doc as 'terms' | 'privacy'].title }}</span>
              <Icon name="chevron" :size="16" class="chev" />
            </RouterLink>
          </nav>
        </template>

        <template v-else-if="step === 'sign'">
          <h1 class="serif">风险揭示书</h1>
          <div class="paper">
            <LegalText :doc="DOCUMENTS.disclosure" />
          </div>
          <label class="field sig">签名
            <input v-model="signature" class="input" autocomplete="off" autocapitalize="off" spellcheck="false"
              :placeholder="session.me?.username" @keydown.enter.prevent="sign" />
          </label>
        </template>

        <template v-else>
          <h1 class="serif">开户完成</h1>
          <dl class="facts receipt">
            <div><dt>账号</dt><dd class="clamp1">{{ session.me?.username }}</dd></div>
            <div><dt>开户时间</dt><dd>{{ when(session.me?.created_at) }}</dd></div>
            <div><dt>初始资金</dt><dd>{{ money(session.me?.initial_cash, 0) }}</dd></div>
          </dl>
          <nav class="rows">
            <RouterLink to="/guide" class="row"><span>新手指南</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
            <RouterLink to="/settings" class="row"><span>身份验证</span><span class="v">财政补贴、新股认购等</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
            <RouterLink to="/assessment/risk" class="row"><span>风险测评</span><span class="v">开通融资融券需要</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          </nav>
        </template>
      </section>
    </Transition>

    <div class="bottom">
      <p v-if="error" class="msg error" role="alert">{{ error }}</p>
      <button v-if="step === 'intro'" type="button" class="btn block cta" @click="advance">下一步</button>
      <button v-else-if="step === 'accept'" type="button" class="btn block cta" :disabled="busy" @click="accept">同意</button>
      <button v-else-if="step === 'sign'" type="button" class="btn block cta" :disabled="busy || !signed" @click="sign">签署</button>
      <button v-else type="button" class="btn block cta" @click="finish()">进入市场</button>
    </div>
  </main>
</template>

<style scoped>
.welcome { min-height: 100vh; min-height: 100dvh; max-width: 520px; margin: 0 auto; padding: 0 20px; display: flex; flex-direction: column; }
.top { display: flex; align-items: center; gap: 6px; min-height: 32px; padding-top: calc(22px + env(safe-area-inset-top, 0px)); }
.dot { flex: 1; height: 3px; background: var(--hair); transition: background-color 0.3s ease; }
.dot.on { background: var(--ink); }
.skip { margin-left: auto; padding: 6px 0 6px 10px; border: 0; background: transparent; font-size: 13px; color: var(--ink-2); }
.body { display: flex; flex-direction: column; gap: 14px; padding: 30px 0 8px; flex: 1; min-height: 0; }
h1 { font-size: 30px; font-weight: 900; line-height: 1.25; }
.lede { font-size: 15px; line-height: 1.7; color: var(--ink-2); text-wrap: pretty; }
.live { margin-top: 10px; border-top: 1px solid var(--rule); border-bottom: 1px solid var(--rule); }
.live li { display: grid; grid-template-columns: 30px minmax(0, 1fr) auto 64px; gap: 12px; align-items: center; padding: 8px 0; }
.live li + li { border-top: 1px solid var(--hair-2); }
.live .t { font-size: 15px; font-weight: 600; }
.live .p { font-size: 14px; text-align: right; }
.live .c { font-size: 13px; text-align: right; }
/* the disclosure reads in its own scroll area, ruled above and below like a printed form */
.paper {
  flex: 1; min-height: 220px; max-height: 52vh; overflow-y: auto; overscroll-behavior: contain;
  border-top: 1px solid var(--rule); border-bottom: 1px solid var(--rule); padding: 14px 2px 18px;
}
.sig { padding-top: 4px; }
.rows { border-top: 1px solid var(--rule); }
.row { display: flex; align-items: center; justify-content: space-between; padding: 14px 0; border-bottom: 1px solid var(--hair-2); font-size: 15px; }
.row .v { margin-left: auto; margin-right: 6px; font-size: 12px; color: var(--muted); }
.chev { color: var(--muted); }
.receipt { border-top: 1px solid var(--rule); margin-bottom: 18px; }
.receipt dd { font-size: 14px; }
.bottom { padding: 18px 0 calc(28px + env(safe-area-inset-bottom, 0px)); display: flex; flex-direction: column; gap: 10px; }
.cta { height: 50px; font-size: 16px; }

.step-enter-active, .step-leave-active { transition: opacity 0.22s ease, transform 0.22s cubic-bezier(0.2, 0.7, 0.2, 1); }
.step-enter-from { opacity: 0; transform: translateX(16px); }
.step-leave-to { opacity: 0; transform: translateX(-16px); }
@media (prefers-reduced-motion: reduce) {
  .step-enter-from, .step-leave-to { transform: none; }
}
</style>
