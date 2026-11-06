<script setup lang="ts">
// Account opening (docs/v2-account.md §7). Right after registering (?new=1): what AniEx is, the risk
// disclosure, the receipt. After a sign-in with documents pending (existing players, a new version):
// accept the updated documents, sign the disclosure. Nothing here places an order.
import { computed, nextTick, ref } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { api, call, errorText } from '../api'
import Icon from '../components/Icon.vue'
import LegalText from '../components/LegalText.vue'
import { money, when } from '../format'
import { DOCUMENTS, isDocKey } from '../legal/documents'
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
          <h1 class="serif">每部作品都是一支股票</h1>
          <p class="lede">公司的营收来自真实观众的观看和讨论。作品越受欢迎，营收越高，股价随大家对营收的预期涨跌。每周日，公司把利润作为分红发给股东。</p>
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
            <RouterLink to="/settings" class="row"><span>身份验证</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
            <RouterLink to="/guide" class="row"><span>新手指南</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
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
.demo { width: 100%; height: auto; margin-top: 8px; }
.demo .b { stroke: var(--base-line); }
.demo .heat { fill: none; stroke: var(--hold-npc); stroke-width: 2; stroke-dasharray: 4 4; }
.demo .px { fill: none; stroke: var(--up); stroke-width: 2.4; stroke-linejoin: round; }
.demo .ep { stroke: var(--accent); stroke-dasharray: 2 3; }
.demo .tag { fill: var(--accent); }
.demo text { font-size: 11px; fill: #fff; font-family: var(--font); }
.legend { display: flex; gap: 16px; font-size: 12px; color: var(--ink-2); }
.legend span { display: flex; align-items: center; gap: 6px; }
.lp { width: 14px; border-top: 2px solid var(--up); }
.lh { width: 14px; border-top: 2px dashed var(--hold-npc); }
/* the disclosure reads in its own scroll area, ruled above and below like a printed form */
.paper {
  flex: 1; min-height: 220px; max-height: 52vh; overflow-y: auto; overscroll-behavior: contain;
  border-top: 1px solid var(--rule); border-bottom: 1px solid var(--rule); padding: 14px 2px 18px;
  scrollbar-width: thin; scrollbar-color: var(--hair) transparent;
}
.sig { padding-top: 4px; }
.rows { border-top: 1px solid var(--rule); }
.row { display: flex; align-items: center; justify-content: space-between; padding: 14px 0; border-bottom: 1px solid var(--hair-2); font-size: 15px; }
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
