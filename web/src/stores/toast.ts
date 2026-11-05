// Short notices at the bottom of the screen (own fills). A toast with the same key replaces the last one.
import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface Toast {
  id: number
  key: string
  text: string
  sub?: string
  tone: 'buy' | 'sell' | 'plain'
}

export const useToast = defineStore('toast', () => {
  const items = ref<Toast[]>([])
  let n = 0
  const timers = new Map<number, number>()

  function show(t: Omit<Toast, 'id'>, ms = 4000) {
    const old = items.value.find((x) => x.key === t.key)
    const id = old?.id ?? ++n
    if (old) Object.assign(old, t)
    else items.value = [...items.value, { ...t, id }].slice(-3)
    window.clearTimeout(timers.get(id))
    timers.set(id, window.setTimeout(() => dismiss(id), ms))
  }
  function dismiss(id: number) {
    items.value = items.value.filter((x) => x.id !== id)
    timers.delete(id)
  }
  return { items, show, dismiss }
})
