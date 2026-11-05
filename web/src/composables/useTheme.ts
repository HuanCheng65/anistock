import { onBeforeUnmount, ref } from 'vue'

/** Reactive counter that bumps when the OS colour scheme changes, so canvas charts can re-read CSS tokens. */
export function useThemeTick() {
  const tick = ref(0)
  const mq = window.matchMedia('(prefers-color-scheme: dark)')
  const on = () => tick.value++
  mq.addEventListener('change', on)
  onBeforeUnmount(() => mq.removeEventListener('change', on))
  return tick
}

export function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}
