// v-flash="value": when the bound number changes, the element's background flashes the up or down tint
// (red up, green down) and fades in ~600 ms. v-flash.neutral flashes a neutral tint (book levels, sizes).
// Nothing happens on first render or when a poll returns the same value. Styles: base.css (.flash-*).
import type { Directive } from 'vue'

const timers = new WeakMap<HTMLElement, number>()
const CLASSES = ['flash-up', 'flash-down', 'flash-neutral']

export function flash(el: HTMLElement, kind: 'up' | 'down' | 'neutral') {
  window.clearTimeout(timers.get(el))
  el.classList.remove(...CLASSES)
  void el.offsetWidth // restart the animation
  el.classList.add('flash-' + kind)
  timers.set(el, window.setTimeout(() => el.classList.remove(...CLASSES), 700))
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
