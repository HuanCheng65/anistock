// Page guides (docs: the onboarding plan): a few steps that point at parts of a page whose ideas the layout cannot
// explain (the order book, how the price moves, the order panel). Each guide shows once per account; the server
// keeps which were seen (`me.onboarding.marks`), and a visitor's browser keeps them until sign-in.
import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { api, call, type BookLevel } from '../api'
import { useSession } from './session'

export type GuideMark = 'guide.stock' | 'guide.order'

export interface GuideStep {
  /** CSS selector of the element to point at; the first visible match is used, a step without one is skipped */
  target: string
  title: string
  body: string
  /** the order-book animation of 涨跌, played on `book()` (no animation when it has no 卖1) */
  demo?: 'tick'
  book?: () => { bids: BookLevel[]; asks: BookLevel[]; last: number } | null
}

/** The book and last price the 涨跌 animation shows in place of the live ones (null: live). */
export interface DemoBook {
  bids: BookLevel[]
  asks: BookLevel[]
  last: number
  /** the last price before the animation */
  from: number
  /** the buy order shown arriving */
  order: { price: number; quantity: number }
  phase: 'order' | 'fill' | 'shift' | 'rest'
}

const LOCAL = 'aniex.guides'

function loadLocal(): string[] {
  try {
    return JSON.parse(localStorage.getItem(LOCAL) ?? '[]') as string[]
  } catch {
    return []
  }
}
function saveLocal(v: string[]) {
  try {
    if (v.length) localStorage.setItem(LOCAL, JSON.stringify(v))
    else localStorage.removeItem(LOCAL)
  } catch {
    /* private mode */
  }
}

export const useGuide = defineStore('guide', () => {
  const session = useSession()
  const steps = ref<GuideStep[]>([])
  const at = ref(-1)
  const mark = ref<GuideMark | null>(null)
  const demo = ref<DemoBook | null>(null)
  /** how the last guide ended: finished (完成) or skipped (跳过) */
  const ended = ref<{ mark: GuideMark; skipped: boolean } | null>(null)
  /** 新手指南 → 重看引导: show the guides again in this tab */
  const replay = ref<Set<GuideMark>>(new Set())
  const local = ref<string[]>(loadLocal())

  function seen(m: GuideMark): boolean {
    if (replay.value.has(m)) return false
    // a server from before onboarding sends no `onboarding` (the web can deploy first): no guides then
    return session.me ? session.me.onboarding?.marks.includes(m) ?? true : local.value.includes(m)
  }

  /** Start `m` unless it was seen, or another guide is running. */
  function start(m: GuideMark, list: GuideStep[]) {
    if (mark.value || seen(m) || !list.length) return
    mark.value = m
    steps.value = list
    at.value = 0
  }

  function record(m: GuideMark) {
    replay.value.delete(m)
    const me = session.me
    if (me) {
      if (!me.onboarding) return
      if (!me.onboarding.marks.includes(m)) me.onboarding = { ...me.onboarding, marks: [...me.onboarding.marks, m] }
      call(api.POST('/api/v1/me/marks/{key}', { params: { path: { key: m } } }))
        .then((r) => (session.me = r))
        .catch(() => {})
    } else if (!local.value.includes(m)) {
      local.value = [...local.value, m]
      saveLocal(local.value)
    }
  }

  /** Close the guide (finished or skipped): it counts as seen either way. */
  function end(skipped = false) {
    if (mark.value) {
      record(mark.value)
      ended.value = { mark: mark.value, skipped }
    }
    mark.value = null
    steps.value = []
    at.value = -1
    demo.value = null
  }

  function next() {
    demo.value = null
    if (at.value < steps.value.length - 1) at.value += 1
    else end()
  }

  /** Leave without recording (the page went away mid-guide): it shows again next time. */
  function abandon() {
    mark.value = null
    steps.value = []
    at.value = -1
    demo.value = null
  }

  function again() {
    replay.value = new Set(['guide.stock', 'guide.order'])
  }

  // a visitor who saw the guides and then signs in: the account keeps them as seen
  watch(
    () => session.me?.id ?? null,
    (id) => {
      if (id == null || !local.value.length) return
      const done = local.value.filter((m) => m === 'guide.stock' || m === 'guide.order') as GuideMark[]
      local.value = []
      saveLocal([])
      for (const m of done) if (!session.me?.onboarding?.marks.includes(m)) record(m)
    },
    { immediate: true },
  )

  return { steps, at, mark, demo, ended, seen, start, next, end, abandon, again }
})
