// Preferences. 自选 is kept on the server for a signed-in player (GET /me/watchlist) and in localStorage before
// sign-in; the first sign-in merges the device's list into the account's and clears it from the device. The rest
// (theme, side panel, order confirmation) stays per device in localStorage.
import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { api, ApiError, call } from '../api'
import { useSession } from './session'
import { useToast } from './toast'

function load<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key)
    return v == null ? fallback : (JSON.parse(v) as T)
  } catch {
    return fallback
  }
}
function save(key: string, v: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(v))
  } catch {
    /* private mode */
  }
}

export interface StockContext {
  key: string // 'watch' | 'hold' | a 股票 filter key
  label: string
  ids: number[]
}

function loadSession<T>(key: string): T | null {
  try {
    const v = sessionStorage.getItem(key)
    return v == null ? null : (JSON.parse(v) as T)
  } catch {
    return null
  }
}

export type Theme = 'system' | 'light' | 'dark'

export function applyTheme(t: Theme) {
  const el = document.documentElement
  if (t === 'system') el.removeAttribute('data-theme')
  else el.setAttribute('data-theme', t)
}

export const usePrefs = defineStore('prefs', () => {
  // the device's 自选 (signed out), and the account's once loaded (null: signed out, not loaded, or a server
  // without the endpoint, when the device's list stays in use)
  const local = ref<number[]>(load('aniex.watchlist', []))
  const server = ref<number[] | null>(null)
  const watchlist = computed(() => server.value ?? local.value)
  const theme = ref<Theme>(load('aniex.theme', 'system'))
  const sideCollapsed = ref<boolean>(load('aniex.sideCollapsed', false))
  // 下单前确认: a summary to confirm before an order is sent (on unless the player turns it off)
  const confirmOrders = ref<boolean>(load('aniex.confirmOrders', true))
  watch(confirmOrders, (v) => save('aniex.confirmOrders', v))
  // the list the player opened a stock from (股票 page filter + sort, 自选, 持仓), for the detail sidebar
  const context = ref<StockContext | null>(loadSession('aniex.context'))
  watch(sideCollapsed, (v) => save('aniex.sideCollapsed', v))
  watch(context, (v) => {
    try {
      sessionStorage.setItem('aniex.context', JSON.stringify(v))
    } catch {
      /* private mode */
    }
  })

  watch(local, (v) => save('aniex.watchlist', v), { deep: true })
  watch(theme, (v) => {
    save('aniex.theme', v)
    applyTheme(v)
  }, { immediate: true })

  const session = useSession()
  let loadedAt = 0
  let loading: Promise<void> | null = null
  /** Load the account's 自选, merging the device's list into it the first time. */
  function syncWatchlist(): Promise<void> {
    if (!session.me) {
      server.value = null
      return Promise.resolve()
    }
    loading ??= (async () => {
      try {
        let items = (await call(api.GET('/api/v1/me/watchlist'))).items
        if (local.value.length) {
          items = (await call(api.POST('/api/v1/me/watchlist/import', { body: { items: local.value } }))).items
          local.value = []
          try {
            localStorage.removeItem('aniex.watchlist')
          } catch {
            /* private mode */
          }
          void session.loadMe().catch(() => {})
        }
        server.value = items
        loadedAt = Date.now()
      } catch {
        // offline, or a server from before /me/watchlist: the device's list stays, merged on a later sign-in
      } finally {
        loading = null
      }
    })()
    return loading
  }
  watch(() => session.me?.id ?? null, (id, was) => {
    if (id !== was) {
      server.value = null
      void syncWatchlist()
    }
  }, { immediate: true })
  // back to a tab left open: another device may have changed the list
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible' && server.value && Date.now() - loadedAt > 60_000) void syncWatchlist()
  })

  const watching = (id: number) => watchlist.value.includes(id)
  async function toggle(id: number) {
    const list = server.value
    const on = watchlist.value.includes(id)
    if (!list) {
      local.value = on ? local.value.filter((x) => x !== id) : [id, ...local.value]
      return
    }
    server.value = on ? list.filter((x) => x !== id) : [id, ...list]
    try {
      const path = { params: { path: { id } } }
      const r = await call(on ? api.DELETE('/api/v1/me/watchlist/{id}', path) : api.PUT('/api/v1/me/watchlist/{id}', path))
      server.value = r.items
      const me = session.me
      if (!on && me?.onboarding && !me.onboarding.watched) me.onboarding = { ...me.onboarding, watched: true, marks: [...me.onboarding.marks, 'watch'] }
    } catch (e) {
      server.value = list
      useToast().show({ key: 'watch', text: e instanceof ApiError && e.code === 'watchlist_full' ? e.message : '无法更新自选', tone: 'plain' })
    }
  }
  return { watchlist, theme, confirmOrders, sideCollapsed, context, watching, toggle, syncWatchlist }
})
