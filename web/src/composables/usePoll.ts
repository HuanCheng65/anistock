import { onBeforeUnmount, onMounted } from 'vue'
import { liveState } from '../live'

/** Run `fn` now and every `ms` while the page is visible. With `liveMs`, the data also arrives over the
 *  WebSocket: while it is connected, poll only every `liveMs` (a resync), and every `ms` when it is down.
 *  A failed run (a server restart, a dropped request) is retried after 2, 4, 8, 16 s, not after the full
 *  interval, which is minutes for the slow blocks. */
export function usePoll(fn: () => Promise<unknown> | unknown, ms = 5000, liveMs?: number) {
  let timer: number | undefined
  let retry: number | undefined
  let fails = 0
  let running = false
  let last = 0

  async function run() {
    if (running || document.hidden) return
    running = true
    last = Date.now()
    window.clearTimeout(retry)
    try {
      await fn()
      fails = 0
    } catch {
      // the view shows its own error state meanwhile
      fails += 1
      retry = window.setTimeout(run, Math.min(ms, 2000 * 2 ** Math.min(fails - 1, 3)))
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
    window.clearTimeout(retry)
    document.removeEventListener('visibilitychange', onVisible)
  })
  return { refresh: run }
}
