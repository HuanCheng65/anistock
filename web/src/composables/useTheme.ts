import { onBeforeUnmount, ref } from 'vue'

/** Bumps when the colour scheme changes (system or the data-theme override), so canvas charts re-read tokens. */
export function useThemeTick() {
  const tick = ref(0)
  const mq = window.matchMedia('(prefers-color-scheme: dark)')
  const on = () => tick.value++
  mq.addEventListener('change', on)
  const mo = new MutationObserver(on)
  mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
  onBeforeUnmount(() => {
    mq.removeEventListener('change', on)
    mo.disconnect()
  })
  return tick
}

export function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}
