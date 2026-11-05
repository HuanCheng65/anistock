import { onBeforeUnmount, onMounted } from 'vue'
import { liveState } from '../live'

/** Run `fn` now and every `ms` while the page is visible. With `liveMs`, the data also arrives over the
 *  WebSocket: while it is connected, poll only every `liveMs` (a resync), and every `ms` when it is down. */
export function usePoll(fn: () => Promise<unknown> | unknown, ms = 5000, liveMs?: number) {
  let timer: number | undefined
  let running = false
  let last = 0

  async function run() {
    if (running || document.hidden) return
    running = true
    last = Date.now()
    try {
      await fn()
    } catch {
      /* the view shows its own error state */
    } finally {
      running = false
    }
  }

  function due() {
    if (liveMs && liveState.value === 'open' && Date.now() - last < liveMs) return
    run()
  }

  function onVisible() {
    if (!document.hidden) run()
  }

  onMounted(() => {
    run()
    timer = window.setInterval(due, ms)
    document.addEventListener('visibilitychange', onVisible)
  })
  onBeforeUnmount(() => {
    window.clearInterval(timer)
    document.removeEventListener('visibilitychange', onVisible)
  })
  return { refresh: run }
}
