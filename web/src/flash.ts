// v-flash="value": when the bound number changes, the element's background flashes the up or down tint
// (red up, green down) and fades in ~600 ms. v-flash.neutral flashes a neutral tint (book levels, sizes).
// Nothing happens on first render or when a poll returns the same value. Styles: base.css (.flash-*).
import type { Directive } from 'vue'

const timers = new WeakMap<HTMLElement, number>()
const CLASSES = ['flash-up', 'flash-down', 'flash-neutral']

// Restarting a CSS animation needs a style flush between removing and re-adding the class. One push flashes
// many cells: they are restarted together in the next frame with a single flush, not one forced layout each.
const queue = new Map<HTMLElement, string>()
let scheduled = false

function restart() {
  scheduled = false
  const batch = [...queue]
  queue.clear()
  for (const [el] of batch) el.classList.remove(...CLASSES)
  void document.body.offsetWidth
  for (const [el, kind] of batch) {
    el.classList.add('flash-' + kind)
    timers.set(el, window.setTimeout(() => el.classList.remove(...CLASSES), 700))
  }
}

export function flash(el: HTMLElement, kind: 'up' | 'down' | 'neutral') {
  if (document.hidden) return // no frames run in a hidden tab; nobody would see it
  window.clearTimeout(timers.get(el))
  queue.set(el, kind)
  if (!scheduled) {
    scheduled = true
    requestAnimationFrame(restart)
  }
}

export const vFlash: Directive<HTMLElement, number | null | undefined> = {
  updated(el, b) {
    const v = b.value
    const o = b.oldValue
    if (v == null || o == null || v === o || Number.isNaN(v)) return
    flash(el, b.modifiers.neutral ? 'neutral' : v > o ? 'up' : 'down')
  },
}

declare module 'vue' {
  interface GlobalDirectives {
    vFlash: typeof vFlash
  }
}
