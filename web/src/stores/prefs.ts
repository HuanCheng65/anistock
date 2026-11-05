// Per-device preferences in localStorage: watchlist, theme, onboarding.
import { defineStore } from 'pinia'
import { ref, watch } from 'vue'

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
  const watchlist = ref<number[]>(load('aniex.watchlist', []))
  const theme = ref<Theme>(load('aniex.theme', 'system'))
  const onboarded = ref<boolean>(load('aniex.onboarded', false))
  const sideCollapsed = ref<boolean>(load('aniex.sideCollapsed', false))
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

  watch(watchlist, (v) => save('aniex.watchlist', v), { deep: true })
  watch(theme, (v) => {
    save('aniex.theme', v)
    applyTheme(v)
  }, { immediate: true })
  watch(onboarded, (v) => save('aniex.onboarded', v))

  const watching = (id: number) => watchlist.value.includes(id)
  function toggle(id: number) {
    const i = watchlist.value.indexOf(id)
    if (i >= 0) watchlist.value.splice(i, 1)
    else watchlist.value.unshift(id)
  }
  return { watchlist, theme, onboarded, sideCollapsed, context, watching, toggle }
})
