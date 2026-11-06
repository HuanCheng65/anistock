<script setup lang="ts">
// /assessment/risk (风险测评) and /assessment/margin-quiz (融资融券知识测试), docs/v2-account.md §5–§6.
// The questions come from the server, which scores them.
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { api, call, errorText } from '../api'
import BackButton from '../components/BackButton.vue'
import BrandMark from '../components/BrandMark.vue'
import type { components } from '../api/schema'
import { useSession } from '../stores/session'

type Assessment = components['schemas']['Assessment']
type RiskResult = components['schemas']['RiskResult']
type QuizResult = components['schemas']['QuizResult']

const route = useRoute()
const router = useRouter()
const session = useSession()
const kind = computed(() => (route.params.kind === 'margin-quiz' ? 'margin-quiz' : 'risk'))
const title = computed(() => (kind.value === 'risk' ? '风险测评' : '融资融券知识测试'))

const paper = ref<Assessment | null>(null)
const answers = ref<(number | null)[]>([])
const risk = ref<RiskResult | null>(null)
const quiz = ref<QuizResult | null>(null)
const busy = ref(false)
const error = ref('')

async function load() {
  error.value = ''
  quiz.value = null
  // a valid result is shown first; 重新测评 starts over
  risk.value = kind.value === 'risk' && session.me?.risk?.valid ? session.me.risk : null
  try {
    paper.value = await call(api.GET('/api/v1/assessments/{kind}', { params: { path: { kind: kind.value } } }))
    answers.value = paper.value.questions.map(() => null)
  } catch (e) {
    error.value = errorText(e, '无法加载题目')
  }
}
watch(kind, load, { immediate: true })

const complete = computed(() => answers.value.length > 0 && answers.value.every((a) => a != null))

async function submit() {
  if (!complete.value || busy.value) return
  busy.value = true
  error.value = ''
  try {
    const r = await call(api.POST('/api/v1/me/assessments/{kind}', {
      params: { path: { kind: kind.value } },
      body: { answers: answers.value as number[] },
    }))
    risk.value = r.risk ?? null
    quiz.value = r.quiz ?? null
    session.loadMe().catch(() => {})
    window.scrollTo({ top: 0 })
  } catch (e) {
    error.value = errorText(e)
  } finally {
    busy.value = false
  }
}
function again() {
  risk.value = null
  quiz.value = null
  answers.value = (paper.value?.questions ?? []).map(() => null)
  window.scrollTo({ top: 0 })
}
/** 2026 年 2 月 4 日, without the year inside this year */
function dateText(d: string): string {
  const [y, m, dd] = d.split('-').map(Number)
  const same = y === new Date().getFullYear()
  return `${same ? '' : `${y} 年 `}${m} 月 ${dd} 日`
}
function done() {
  if (window.history.state?.back) router.back()
  else router.replace('/me')
}
</script>

<template>
  <main class="page doc">
    <header class="pagehead back">
      <BackButton />
      <h1>{{ title }}</h1>
    </header>

    <Transition name="swap" mode="out-in">
      <!-- 风险测评 result: a card to keep -->
      <section v-if="risk" key="risk" class="result gutter">
        <div class="card">
          <div class="mark"><BrandMark :size="22" /><span>安易 · 风险测评</span></div>
          <div class="lv serif">{{ risk.level }}</div>
          <div class="nm serif">{{ risk.name }}</div>
          <p class="motto serif">{{ risk.motto }}</p>
          <p class="desc">{{ risk.description }}</p>
          <p class="until">有效期至 {{ dateText(risk.valid_until) }}</p>
        </div>
        <div class="acts">
          <button type="button" class="btn block" @click="done">完成</button>
          <button type="button" class="btn ghost block" @click="again">重新测评</button>
        </div>
      </section>

      <!-- knowledge test result: the score, then why for each question missed -->
      <section v-else-if="quiz && paper" key="quiz" class="result gutter">
        <div class="score">
          <span class="num big">{{ quiz.score }}<span class="of"> / {{ quiz.total }}</span></span>
          <span :class="quiz.passed ? 'pass' : 'fail'">{{ quiz.passed ? '已通过' : `未通过，答对 ${quiz.pass_score} 题即可通过` }}</span>
        </div>
        <ol class="review">
          <li v-for="(r, i) in quiz.review" :key="i" :class="{ wrong: !r.correct }">
            <p class="qt">{{ paper.questions[i].text }}</p>
            <p v-if="!r.correct" class="ans">{{ paper.questions[i].options[r.answer] }}</p>
            <p v-if="!r.correct" class="why">{{ r.explanation }}</p>
          </li>
        </ol>
        <div class="acts">
          <button type="button" class="btn block" @click="quiz.passed ? done() : again()">{{ quiz.passed ? '完成' : '重新测试' }}</button>
        </div>
      </section>

      <form v-else-if="paper" key="paper" class="paper gutter" @submit.prevent="submit">
        <fieldset v-for="(q, i) in paper.questions" :key="i" class="q">
          <legend><span class="num n">{{ i + 1 }}</span>{{ q.text }}</legend>
          <label v-for="(o, j) in q.options" :key="j" class="opt" :class="{ on: answers[i] === j }">
            <input v-model="answers[i]" type="radio" :name="`q${i}`" :value="j" />
            <span>{{ o }}</span>
          </label>
        </fieldset>
        <p v-if="error" class="msg error" role="alert">{{ error }}</p>
        <button type="submit" class="btn block submit" :disabled="!complete || busy || !session.me">提交</button>
      </form>
      <p v-else-if="error" key="err" class="msg error gutter loaderr">{{ error }}</p>
    </Transition>
  </main>
</template>

<style scoped>
.paper { display: flex; flex-direction: column; gap: 6px; padding-top: 6px; padding-bottom: 24px; }
.q { border: 0; margin: 0; padding: 14px 0 8px; border-bottom: 1px solid var(--hair-2); display: flex; flex-direction: column; gap: 6px; }
.q legend { padding: 0 0 8px; font-size: 15px; line-height: 1.5; font-weight: 600; display: flex; gap: 10px; }
.q .n { color: var(--muted); font-weight: 400; min-width: 1.2em; }
.opt {
  display: flex; align-items: center; gap: 10px; min-height: 44px; padding: 8px 12px;
  border: 1px solid var(--hair); font-size: 14px; color: var(--ink-2); cursor: pointer;
  transition: border-color 0.15s ease, background-color 0.15s ease, color 0.15s ease;
}
.opt input { position: absolute; opacity: 0; pointer-events: none; }
.opt.on { border-color: var(--ink); color: var(--ink); background: var(--ground-2); font-weight: 600; }
.opt:has(input:focus-visible) { outline: 2px solid var(--ink); outline-offset: 2px; }
.submit { height: 48px; font-size: 16px; margin-top: 14px; }

.result { padding-top: 20px; padding-bottom: 24px; display: flex; flex-direction: column; gap: 18px; }
.card {
  border-top: 3px solid var(--ink); border-bottom: 1px solid var(--rule); padding: 18px 0 20px;
  display: flex; flex-direction: column; gap: 6px;
}
.mark { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--muted); margin-bottom: 10px; }
.lv { font-size: 64px; font-weight: 900; line-height: 1; }
.nm { font-size: 24px; font-weight: 900; }
.motto { font-size: 18px; color: var(--ink); margin-top: 10px; }
.desc { font-size: 14px; color: var(--ink-2); line-height: 1.7; }
.until { font-size: 12px; color: var(--muted); margin-top: 8px; }
.acts { display: flex; flex-direction: column; gap: 10px; }
.acts .btn { height: 48px; font-size: 16px; }

.score { display: flex; flex-direction: column; gap: 6px; padding-bottom: 14px; border-bottom: 1px solid var(--rule); }
.big { font-size: 48px; font-weight: 600; line-height: 1.1; }
.of { font-size: 20px; color: var(--muted); }
.pass { font-size: 15px; font-weight: 700; }
.fail { font-size: 14px; color: var(--danger); }
.review { display: flex; flex-direction: column; }
.review li { padding: 10px 0; border-bottom: 1px solid var(--hair-2); display: flex; flex-direction: column; gap: 4px; }
.review li:not(.wrong) .qt { color: var(--muted); }
.qt { font-size: 14px; line-height: 1.6; }
.ans { font-size: 14px; font-weight: 700; }
.why { font-size: 13px; color: var(--ink-2); line-height: 1.6; }
.loaderr { padding-top: 16px; }

.swap-enter-active, .swap-leave-active { transition: opacity 0.2s ease, transform 0.2s cubic-bezier(0.2, 0.7, 0.2, 1); }
.swap-enter-from { opacity: 0; transform: translateY(8px); }
.swap-leave-to { opacity: 0; }
@media (prefers-reduced-motion: reduce) { .swap-enter-from { transform: none; } }
</style>
