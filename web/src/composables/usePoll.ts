import { onBeforeUnmount, onMounted } from 'vue'

/** Run `fn` now and every `ms` while the page is visible. */
export function usePoll(fn: () => Promise<unknown> | unknown, ms = 5000) {
  let timer: number | undefined
  let running = false

  async function run() {
    if (running || document.hidden) return
    running = true
    try {
      await fn()
    } catch {
      /* the view shows its own error state */
    } finally {
      running = false
    }
  }

  function onVisible() {
    if (!document.hidden) run()
  }

  onMounted(() => {
    run()
    timer = window.setInterval(run, ms)
    document.addEventListener('visibilitychange', onVisible)
  })
  onBeforeUnmount(() => {
    window.clearInterval(timer)
    document.removeEventListener('visibilitychange', onVisible)
  })
  return { refresh: run }
}
