import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, ApiError, type Me, type Status } from '../api'

export const useSession = defineStore('session', () => {
  const me = ref<Me | null>(null)
  const status = ref<Status | null>(null)
  const checked = ref(false)

  async function loadMe() {
    try {
      me.value = await api.get<Me>('/api/me')
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) me.value = null
      else throw e
    } finally {
      checked.value = true
    }
  }

  async function loadStatus() {
    try {
      status.value = await api.get<Status>('/api/status')
    } catch {
      /* keep the last known status */
    }
  }

  async function login(username: string, password: string) {
    await api.post('/api/login', { username, password })
    await loadMe()
  }

  async function register(username: string, password: string, invite: string) {
    await api.post('/api/register', { username, password, invite })
    await loadMe()
  }

  async function logout() {
    await api.post('/api/logout')
    me.value = null
  }

  return { me, status, checked, loadMe, loadStatus, login, register, logout }
})
