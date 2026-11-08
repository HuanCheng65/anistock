<script setup lang="ts">
// The page guide on top of everything: the element of the current step is cut out of a dimmed layer, the card
// with the step sits below it (above it when there is no room). The 涨跌 step plays the book animation: a buy at
// 卖1 for a little more than 卖1 holds, so 卖1 fills, the price moves to it, and the rest waits as the new 买1.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { money, qty as fmtQty } from '../format'
import { useGuide, type GuideStep } from '../stores/guide'

const guide = useGuide()
const step = computed<GuideStep | null>(() => (guide.at >= 0 ? guide.steps[guide.at] ?? null : null))
const box = ref<{ top: number; left: number; width: number; height: number } | null>(null)
const cardTop = ref(0)
const cardLeft = ref(0)
const card = ref<HTMLElement | null>(null)
const tagAt = ref<{ top: number; left: number; right?: boolean } | null>(null)
const lastAt = ref<{ top: number; left: number } | null>(null)
const reduced = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches

function visible(sel: string): HTMLElement | null {
  for (const el of document.querySelectorAll<HTMLElement>(sel)) if (el.getClientRects().length) return el
  return null
}

let target: HTMLElement | null = null
function measure() {
  if (!target || !target.isConnected) return
  const r = target.getBoundingClientRect()
  const pad = 4
  const vw = window.innerWidth, vh = window.innerHeight
  box.value = { top: r.top - pad, left: Math.max(2, r.left - pad), width: Math.min(vw - 4, r.width + pad * 2), height: r.height + pad * 2 }
  const ch = card.value?.offsetHeight ?? 160
  const cw = Math.min(360, vw - 24)
  const below = r.bottom + pad + 10
  cardTop.value = below + ch < vh - 8 ? below : Math.max(8, r.top - pad - ch - 10)
  cardLeft.value = Math.min(Math.max(12, r.left), vw - cw - 12)
  // the animation's labels: by 卖1 and 买1
  const a1 = visible('[data-guide="ask1"]'), b1 = visible('[data-guide="bid1"]')
  if (guide.demo && a1) {
    const ra = a1.getBoundingClientRect()
    const rb = b1?.getBoundingClientRect()
    const split = !!rb && Math.abs(ra.top - rb.top) < 6
    tagAt.value = split ? { top: ra.top - 24, left: ra.left } : { top: ra.top, left: ra.left - 8, right: true }
    lastAt.value = split && rb ? { top: rb.top - 24, left: rb.left } : null
  } else {
    tagAt.value = null
    lastAt.value = null
  }
}

let frame = 0
function remeasure() {
  cancelAnimationFrame(frame)
  frame = requestAnimationFrame(measure)
}

let timers: number[] = []
function clearTimers() {
  timers.forEach((t) => window.clearTimeout(t))
  timers = []
}

function play() {
  clearTimers()
  guide.demo = null
  const s = step.value
  const b = s?.book?.()
  if (!b || !b.asks.length) return
  const a1 = b.asks[0]
  const extra = Math.max(1, Math.round(a1.quantity / 4))
  const later = (ms: number, f: () => void) => timers.push(window.setTimeout(f, reduced ? 0 : ms))
  const asks = b.asks.map((l) => ({ ...l }))
  guide.demo = { bids: b.bids, asks, last: b.last, from: b.last, order: { price: a1.price, quantity: a1.quantity + extra }, phase: 'order' }
  remeasure()
  const set = (f: (d: NonNullable<typeof guide.demo>) => void) => {
    if (guide.demo) f(guide.demo)
  }
  later(600, () => set((d) => (d.phase = 'fill')))
  ;[0.75, 0.5, 0.25, 0].forEach((k, i) =>
    later(700 + i * 140, () => set((d) => (d.asks = [{ ...a1, quantity: Math.round(a1.quantity * k) }, ...d.asks.slice(1)]))),
  )
  later(1150, () => set((d) => (d.last = a1.price)))
  later(2000, () => set((d) => {
    d.asks = d.asks.slice(1)
    d.phase = 'shift'
  }))
  later(2700, () => set((d) => {
    d.bids = [{ price: a1.price, quantity: extra, orders: 1 }, ...d.bids]
    d.phase = 'rest'
  }))
  later(2750, remeasure)
}

async function show() {
  clearTimers()
  guide.demo = null
  const s = step.value
  if (!s) {
    target = null
    box.value = null
    return
  }
  target = visible(s.target)
  if (!target) {
    guide.next()
    return
  }
  const r = target.getBoundingClientRect()
  if (r.top < 60 || r.bottom > window.innerHeight - 200) {
    target.scrollIntoView({ block: 'center', behavior: reduced ? 'auto' : 'smooth' })
    await new Promise((f) => setTimeout(f, reduced ? 0 : 320))
  }
  await nextTick()
  measure()
  await nextTick()
  measure()
  if (s.demo === 'tick') play()
}

watch(step, show)
watch(() => guide.demo?.phase, remeasure)
onMounted(() => {
  window.addEventListener('resize', remeasure)
  window.addEventListener('scroll', remeasure, { passive: true, capture: true })
})
onBeforeUnmount(() => {
  clearTimers()
  window.removeEventListener('resize', remeasure)
  window.removeEventListener('scroll', remeasure, { capture: true })
})

const n = computed(() => guide.steps.length)
const last = computed(() => guide.at === n.value - 1)
</script>

<template>
  <div v-if="step && box" class="guide" role="dialog" aria-modal="true" :aria-label="step.title">
    <div class="spot" :style="{ top: `${box.top}px`, left: `${box.left}px`, width: `${box.width}px`, height: `${box.height}px` }"></div>
    <div v-if="guide.demo && tagAt" class="tag num" :class="{ right: tagAt.right }"
         :style="tagAt.right ? { top: `${tagAt.top}px`, right: `calc(100vw - ${tagAt.left}px)` } : { top: `${tagAt.top}px`, left: `${tagAt.left}px` }">
      <span class="sans">买入</span> {{ fmtQty(guide.demo.order.quantity) }} <span class="sans">股</span> · {{ money(guide.demo.order.price) }}
    </div>
    <div v-if="guide.demo && lastAt" class="lastp num" :style="{ top: `${lastAt.top}px`, left: `${lastAt.left}px` }">
      <span class="sans k">最新</span><span :key="guide.demo.last" class="v" :class="{ moved: guide.demo.last !== guide.demo.from }">{{ money(guide.demo.last) }}</span>
    </div>
    <div ref="card" class="card" :style="{ top: `${cardTop}px`, left: `${cardLeft}px` }">
      <div class="head">
        <h2>{{ step.title }}</h2>
        <span class="num count">{{ guide.at + 1 }}/{{ n }}</span>
      </div>
      <p>{{ step.body }}</p>
      <div class="acts">
        <button v-if="step.demo" type="button" class="replay" @click="play">重播</button>
        <button v-if="!last" type="button" class="skip" @click="guide.end(true)">跳过</button>
        <button type="button" class="btn small" @click="guide.next()">{{ last ? '完成' : '继续' }}</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.guide { position: fixed; inset: 0; z-index: 90; }
.spot {
  position: absolute; outline: 2px solid var(--ground); box-shadow: 0 0 0 200vmax rgba(20, 19, 16, 0.46);
  transition: top var(--dur-3) var(--ease-out), left var(--dur-3) var(--ease-out), width var(--dur-3) var(--ease-out), height var(--dur-3) var(--ease-out);
  pointer-events: none;
}
.card {
  position: absolute; width: min(360px, calc(100vw - 24px)); background: var(--ground); color: var(--ink);
  border-top: 2px solid var(--ink); padding: 12px 14px;
  transition: top var(--dur-3) var(--ease-out), left var(--dur-3) var(--ease-out);
}
.head { display: flex; align-items: baseline; gap: 8px; }
h2 { font-family: var(--font-serif); font-size: 17px; font-weight: 900; }
.count { margin-left: auto; font-size: 12px; color: var(--muted); }
p { margin-top: 6px; font-size: 14px; line-height: 1.7; color: var(--ink-2); text-wrap: pretty; }
.acts { display: flex; justify-content: flex-end; align-items: center; gap: 16px; margin-top: 10px; }
.skip, .replay { border: 0; background: none; padding: 0; font-size: 13px; color: var(--ink-2); }
.replay { color: var(--accent); margin-right: auto; }
.tag {
  position: absolute; padding: 3px 8px; background: var(--t-up); color: var(--t-on-up); font-size: 12px; font-weight: 700;
  white-space: nowrap; animation: tag-in var(--dur-2) var(--ease-out);
}
.lastp { position: absolute; padding: 2px 8px; background: var(--t-bg); color: var(--t-ink); font-size: 13px; outline: 1px solid var(--t-line); }
.lastp .k { font-size: 12px; color: var(--t-ink-2); margin-right: 6px; }
.lastp .v.moved { color: var(--t-up); animation: moved 0.9s ease-out; }
.sans { font-family: var(--font); }
@keyframes tag-in { from { opacity: 0; transform: translateX(-10px); } }
@keyframes moved { from { background-color: var(--t-flash-up); } }
@media (prefers-reduced-motion: reduce) {
  .spot, .card { transition: none; }
  .tag, .lastp .v.moved { animation: none; }
}
</style>
