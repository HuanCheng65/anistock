// The signed-in player (`Me`), the market status, the game rules and the leaderboards.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, ApiError, call, type Leaderboard, type MarketStatus, type Me, type Rules } from '../api'
import { onLive } from '../live'

export type Period = 'all' | 'week' | 'season'

export const useSession = defineStore('session', () => {
  const me = ref<Me | null>(null)
  const status = ref<MarketStatus | null>(null)
  const rules = ref<Rules | null>(null)
  const checked = ref(false)
  const boards = ref<Partial<Record<Period, Leaderboard>>>({})

  async function loadMe() {
    try {
      me.value = await call(api.GET('/api/v1/me'))
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) me.value = null
      else throw e
    } finally {
      checked.value = true
    }
  }

  async function loadStatus() {
    try {
      status.value = await call(api.GET('/api/v1/market/status'))
    } catch {
      /* keep the last known status */
    }
  }

  async function loadRules() {
    if (rules.value) return
    try {
      rules.value = await call(api.GET('/api/v1/rules'))
    } catch {
      /* pages fall back to the defaults they know */
    }
  }

  async function loadLeaders(period: Period = 'all') {
    boards.value[period] = await call(api.GET('/api/v1/leaderboard', { params: { query: { period } } }))
  }

  async function login(username: string, password: string) {
    me.value = await call(api.POST('/api/v1/auth/login', { body: { username, password } }))
    checked.value = true
  }

  async function register(username: string, password: string, invite: string) {
    me.value = await call(api.POST('/api/v1/auth/register', { body: { username, password, invite } }))
    checked.value = true
  }

  async function logout() {
    await call(api.POST('/api/v1/auth/logout'))
    me.value = null
  }

  // the `time` heartbeat replaces polling /market/status
  onLive('time', (m) => {
    const s = status.value
    if (!s) return
    s.now = m.now
    s.trading_day = m.trading_day
    s.phase = m.phase
    s.phase_ends_at = m.phase_ends_at
    s.last_closed_day = m.last_closed_day
  })

  return { me, status, rules, checked, boards, loadMe, loadStatus, loadRules, loadLeaders, login, register, logout }
})
