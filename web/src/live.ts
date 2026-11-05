// One shared WebSocket to /api/v1/ws (docs/api-v1.md §5; server: backend/src/live.rs). Topics are
// reference-counted: views subscribe on mount and leave on unmount; they are re-sent after every
// reconnect (news with `since`). Frames are NDJSON. While the socket is down, polling takes over (usePoll).
//
// - `time` arrives every 20 s on every connection: 40 s of silence means the connection is dead.
// - state topics (`quotes`, `stock:<id>`) carry a `seq` per topic; a gap asks the server to `resync`.
// - close 4029 (too many connections from this address): no automatic reconnect; 4001 (session ended):
//   reconnect, the server then treats the socket as anonymous.
import { onBeforeUnmount, onMounted, ref, watch, type Ref } from 'vue'
import type { WsServerMessage } from './api'
import { noteServerVersion } from './version'

export type LiveState = 'connecting' | 'open' | 'down'
export const liveState = ref<LiveState>('connecting')

export type Msg<T extends WsServerMessage['t']> = Extract<WsServerMessage, { t: T }>
type Handler = (m: any) => void
const handlers = new Map<string, Set<Handler>>()
const topics = new Map<string, number>()
let ws: WebSocket | null = null
let retry = 0
let timer: number | undefined
let started = false
let lastMsg = 0
let watchdog: number | undefined
let blocked = false
/** last seq per state topic ('quotes', 'stock:<id>'), and the last news id seen */
const seqs = new Map<string, number>()
let lastNews = 0

const BACKOFF = [1000, 2000, 5000, 10000]

function send(o: unknown) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(o))
}

function stateTopic(m: any): string | null {
  if (m.t === 'quotes') return 'quotes'
  if (m.t === 'stock') return `stock:${m.id}`
  return null
}

function dispatch(m: any) {
  if (m.t === 'hello') noteServerVersion(m.server_version)
  const topic = stateTopic(m)
  if (topic) {
    const prev = seqs.get(topic)
    if (m.full) seqs.set(topic, m.seq)
    else if (prev != null && m.seq <= prev) return // older than the full snapshot already applied
    else if (prev != null && m.seq !== prev + 1) {
      seqs.delete(topic)
      send({ resync: [topic] })
      return
    } else seqs.set(topic, m.seq)
  }
  if (m.t === 'news' && typeof m.seq === 'number') lastNews = Math.max(lastNews, m.seq)
  handlers.get(m.t)?.forEach((h) => {
    try {
      h(m)
    } catch (e) {
      console.error(e)
    }
  })
}

function subscribeAll() {
  if (!topics.size) return
  const since: Record<string, number> = {}
  if (lastNews && topics.has('news')) since.news = lastNews
  send({ sub: [...topics.keys()], since })
}

function connect() {
  window.clearTimeout(timer)
  if (blocked) return
  const url = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/v1/ws`
  let sock: WebSocket
  try {
    sock = new WebSocket(url)
  } catch {
    return schedule()
  }
  ws = sock
  seqs.clear()
  sock.onopen = () => {
    retry = 0
    lastMsg = Date.now()
    liveState.value = 'open'
    subscribeAll()
  }
  sock.onmessage = (ev) => {
    lastMsg = Date.now()
    for (const line of String(ev.data).split('\n')) {
      if (!line) continue
      let m: any
      try {
        m = JSON.parse(line)
      } catch {
        continue
      }
      dispatch(m)
    }
  }
  sock.onclose = (ev) => {
    if (ws !== sock) return
    ws = null
    liveState.value = 'down'
    if (ev.code === 4029) {
      blocked = true
      return
    }
    schedule()
  }
}

function schedule() {
  const base = BACKOFF[Math.min(retry, BACKOFF.length - 1)]
  const ms = Math.min(10000, base * (0.75 + Math.random() * 0.5))
  retry++
  timer = window.setTimeout(connect, ms)
}

/** Open the socket once (App.vue). */
export function startLive() {
  if (started) return
  started = true
  connect()
  // no message for 40 s (the server sends `time` every 20 s): the connection is gone
  watchdog = window.setInterval(() => {
    if (ws && ws.readyState === WebSocket.OPEN && Date.now() - lastMsg > 40000) {
      const old = ws
      ws = null
      liveState.value = 'down'
      old.close()
      retry = 0
      connect()
    }
  }, 5000)
  document.addEventListener('visibilitychange', () => {
    // come back at once after the tab was hidden or the machine slept
    if (!document.hidden && !ws && !blocked) {
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
  blocked = false
  liveState.value = 'connecting'
  connect()
}

export function onLive<T extends WsServerMessage['t']>(type: T, h: (m: Msg<T>) => void): () => void {
  let s = handlers.get(type)
  if (!s) handlers.set(type, (s = new Set()))
  s.add(h as Handler)
  return () => s!.delete(h as Handler)
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
    seqs.delete(topic)
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

export function stopLive() {
  window.clearInterval(watchdog)
}
