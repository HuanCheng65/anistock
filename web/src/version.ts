// New server versions (docs/api-v1.md §7.4). The build knows its own version (backend/Cargo.toml, via
// vite `define`); every HTTP response carries `AniEx-Version` and the WebSocket `hello` carries
// `server_version`. When they differ, the page reloads at once unless the player is in the middle of
// something (text in an order form, an open dialog): then a banner offers the reload.
import { reactive, ref } from 'vue'

declare const __APP_VERSION__: string
export const APP_VERSION: string = typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : 'dev'

/** Keys of the places with unsaved input or an open dialog. */
const editing = reactive(new Set<string>())
export const updateReady = ref(false)
let pending: string | null = null

export function markEditing(key: string, on: boolean) {
  if (on) editing.add(key)
  else editing.delete(key)
  if (!on && pending && editing.size === 0) tryReload()
}

function tryReload() {
  if (!pending) return
  // reload once per server version; if the page still differs after that, keep the banner
  let done: string | null = null
  try {
    done = sessionStorage.getItem('aniex.reloadedFor')
  } catch {
    /* private mode */
  }
  if (done === pending) {
    updateReady.value = true
    return
  }
  if (editing.size) {
    updateReady.value = true
    return
  }
  try {
    sessionStorage.setItem('aniex.reloadedFor', pending)
  } catch {
    /* private mode */
  }
  location.reload()
}

export function noteServerVersion(v: string) {
  if (!v || APP_VERSION === 'dev' || v === APP_VERSION || pending === v) return
  pending = v
  tryReload()
}

export function reloadNow() {
  location.reload()
}
