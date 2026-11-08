<script setup lang="ts">
// 入门步骤: a new player's first steps, at the top of 市场 until all are done or the card is closed. Each step is
// read from the account (me.onboarding) and points to where it is done; nothing here acts for the player.
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { api, call } from '../api'
import { useSession } from '../stores/session'
import Icon from './Icon.vue'

const session = useSession()
const closing = ref(false)
const ob = computed(() => session.me?.onboarding ?? null)
const steps = computed(() => {
  const o = ob.value
  if (!o) return []
  return [
    { key: 'signed', label: '签署风险揭示书', done: o.signed, to: { name: 'welcome' }, act: '去签署' },
    { key: 'watched', label: '加入自选', done: o.watched, to: '/stocks', act: '查看行情' },
    { key: 'traded', label: '完成首笔交易', done: o.traded, to: '/guide#first-trade', act: '查看说明' },
    { key: 'verified', label: '完成身份验证', done: o.verified, to: '/settings', act: '去验证' },
    { key: 'dividend', label: '收到首次分红', done: o.dividend, to: null, act: '周日发放' },
  ]
})
const done = computed(() => steps.value.filter((x) => x.done).length)
const show = computed(() => !!ob.value && !ob.value.marks.includes('steps.closed') && done.value < steps.value.length)

async function close() {
  const me = session.me
  if (!me || closing.value) return
  closing.value = true
  me.onboarding = { ...me.onboarding, marks: [...me.onboarding.marks, 'steps.closed'] }
  try {
    session.me = await call(api.POST('/api/v1/me/marks/{key}', { params: { path: { key: 'steps.closed' } } }))
  } catch {
    /* hidden for now; it comes back with the next /me if the mark did not reach the server */
  } finally {
    closing.value = false
  }
}
</script>

<template>
  <section v-if="show" class="steps" aria-labelledby="steps-h">
    <div class="head">
      <h2 id="steps-h">入门步骤</h2>
      <span class="num n">{{ done }}/{{ steps.length }}</span>
      <button type="button" class="x" @click="close">关闭</button>
    </div>
    <ol>
      <li v-for="x in steps" :key="x.key" :class="{ done: x.done }">
        <span class="mk"><Icon v-if="x.done" name="check" :size="12" /></span>
        <span class="s">{{ x.label }}</span>
        <template v-if="!x.done">
          <RouterLink v-if="x.to" :to="x.to" class="a">{{ x.act }}</RouterLink>
          <span v-else class="w">{{ x.act }}</span>
        </template>
      </li>
    </ol>
  </section>
</template>

<style scoped>
.steps { padding: 14px 0 4px; border-bottom: 1px solid var(--rule); }
.head { display: flex; align-items: baseline; gap: 10px; padding-bottom: 8px; border-bottom: 1px solid var(--rule); }
h2 { font-family: var(--font-serif); font-size: 18px; font-weight: 900; }
.n { font-size: 12px; color: var(--muted); }
.x { margin-left: auto; border: 0; background: none; padding: 0; font-size: 13px; color: var(--ink-2); }
li { display: grid; grid-template-columns: 22px minmax(0, 1fr) auto; gap: 8px; align-items: center; padding: 10px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; }
li:last-child { border-bottom: 0; }
.mk { width: 16px; height: 16px; border: 1px solid var(--base-line); display: grid; place-items: center; }
li.done .mk { background: var(--ink); border-color: var(--ink); color: var(--ground); }
li.done .s { color: var(--muted); }
.a { font-size: 13px; color: var(--accent); white-space: nowrap; }
.w { font-size: 12px; color: var(--muted); white-space: nowrap; }
@media (min-width: 1024px) {
  .steps { padding-top: 24px; border-bottom: 0; }
  ol { display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); column-gap: 24px; }
  li { border-bottom: 0; grid-template-columns: 22px minmax(0, 1fr); row-gap: 2px; }
  li .a, li .w { grid-column: 2; }
}
</style>
