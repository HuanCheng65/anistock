import { onBeforeUnmount, ref } from 'vue'

/** Reactive media query (shared listeners per query). */
export function useMedia(query: string) {
  const mq = window.matchMedia(query)
  const matches = ref(mq.matches)
  const on = () => (matches.value = mq.matches)
  mq.addEventListener('change', on)
  onBeforeUnmount(() => mq.removeEventListener('change', on))
  return matches
}

export const DESK = '(min-width: 1024px)'
