// One shared WebSocket to /api/ws (server: backend/src/live.rs). Topics are reference-counted, so views
// subscribe on mount and leave on unmount; they are re-sent after every reconnect. Frames are NDJSON
// (several JSON messages per frame). While the socket is down, polling takes over (usePoll).
import { onBeforeUnmount, onMounted, ref, watch, type Ref } from 'vue'

export type LiveState = 'connecting' | 'open' | 'down'
export const liveState = ref<LiveState>('connecting')

type Handler = (m: any) => void
const handlers = new Map<string, Set<Handler>>()
const topics = new Map<string, number>()
let ws: WebSocket | null = null
let retry = 0
let timer: number | undefined
let started = false

function send(o: unknown) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(o))
}

function connect() {
  window.clearTimeout(timer)
  const url = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/ws`
  let sock: WebSocket
  try {
    sock = new WebSocket(url)
  } catch {
    return schedule()
  }
  ws = sock
  sock.onopen = () => {
    retry = 0
    liveState.value = 'open'
    if (topics.size) send({ sub: [...topics.keys()] })
  }
  sock.onmessage = (ev) => {
    for (const line of String(ev.data).split('\n')) {
      if (!line) continue
      let m: any
      try {
        m = JSON.parse(line)
      } catch {
        continue
      }
      handlers.get(m.t)?.forEach((h) => {
        try {
          h(m)
        } catch (e) {
          console.error(e)
        }
      })
    }
  }
  sock.onclose = () => {
    if (ws !== sock) return
    ws = null
    liveState.value = 'down'
    schedule()
  }
}

function schedule() {
  // 1 s, 2 s, 4 s … up to 30 s, with jitter
  const ms = Math.min(30000, 1000 * 2 ** retry) * (0.75 + Math.random() * 0.5)
  retry = Math.min(retry + 1, 6)
  timer = window.setTimeout(connect, ms)
}

/** Open the socket once (App.vue). */
export function startLive() {
  if (started) return
  started = true
  connect()
  document.addEventListener('visibilitychange', () => {
    // come back at once after the tab was hidden or the machine slept
    if (!document.hidden && !ws) {
      retry = 0
      connect()
    }
  })
}

/** Reconnect, e.g. after login or logout so the server sees the new session cookie. */
export function reconnectLive() {
  if (!started) return
  const old = ws
  ws = null
  old?.close()
  retry = 0
  liveState.value = 'connecting'
  connect()
}

export function onLive(type: string, h: Handler): () => void {
  let s = handlers.get(type)
  if (!s) handlers.set(type, (s = new Set()))
  s.add(h)
  return () => s!.delete(h)
}

export function subscribe(topic: string) {
  const n = topics.get(topic) ?? 0
  topics.set(topic, n + 1)
  if (n === 0) send({ sub: [topic] })
}

export function unsubscribe(topic: string) {
  const n = topics.get(topic) ?? 0
  if (n <= 1) {
    topics.delete(topic)
    send({ unsub: [topic] })
  } else topics.set(topic, n - 1)
}

/** Hold a topic while the component is mounted; `topic` may change (stock page → next stock). */
export function useTopic(topic: string | Ref<string | null>) {
  let cur: string | null = null
  const set = (t: string | null) => {
    if (t === cur) return
    if (cur) unsubscribe(cur)
    cur = t
    if (cur) subscribe(cur)
  }
  if (typeof topic === 'string') {
    onMounted(() => set(topic))
  } else {
    onMounted(() => set(topic.value))
    watch(topic, (t) => set(t))
  }
  onBeforeUnmount(() => set(null))
}
