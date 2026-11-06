<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, watch } from 'vue'
import { RouterView, useRoute } from 'vue-router'
import BottomNav from './components/BottomNav.vue'
import DeskHeader from './components/DeskHeader.vue'
import AgreementBanner from './components/AgreementBanner.vue'
import MarginBanner from './components/MarginBanner.vue'
import Toaster from './components/Toaster.vue'
import UpdateBanner from './components/UpdateBanner.vue'
import { liveState, reconnectLive, startLive, subscribe, unsubscribe } from './live'
import { useAccount } from './stores/account'
import { useCredit } from './stores/credit'
import { useMarket } from './stores/market'
import { usePrefs } from './stores/prefs'
import { useSession } from './stores/session'
import { useStock } from './stores/stock'

const route = useRoute()
const session = useSession()
const market = useMarket()
useAccount() // applies `me` pushes
useCredit() // applies `credit` pushes (追保, 强制平仓)
useStock() // applies `stock:<id>` pushes
usePrefs() // applies the saved theme
const bare = computed(() => !!route.meta.bare)

// one WebSocket for the whole app: quotes and news everywhere, `me` while signed in
subscribe('quotes')
subscribe('news')
startLive()
let meId: number | null = null
let baseline = false
watch(
  () => [session.checked, session.me?.id ?? null] as const,
  ([checked, id]) => {
    if (!checked) return
    if (!baseline) {
      // the first answer of /api/v1/me: the socket opened with this session already
      baseline = true
      meId = id
      if (id != null) subscribe('me')
      return
    }
    if (id === meId) return
    if (meId != null) unsubscribe('me')
    meId = id
    if (id != null) subscribe('me')
    // login or logout: the server reads the session cookie when the socket opens
    reconnectLive()
  },
)

// the status follows the WebSocket's `time` messages; a slow poll covers the time the socket is down
let timer: number | undefined
function tick() {
  if (document.hidden) return
  if (liveState.value !== 'open' || !session.status) session.loadStatus()
  market.refreshIndices()
}
onMounted(() => {
  session.loadStatus()
  session.loadRules()
  tick()
  timer = window.setInterval(tick, 30000)
})
onBeforeUnmount(() => window.clearInterval(timer))
</script>

<template>
  <DeskHeader v-if="!bare" />
  <MarginBanner v-if="!bare" />
  <AgreementBanner v-if="!bare" />
  <RouterView v-slot="{ Component }">
    <component :is="Component" :key="route.path" />
  </RouterView>
  <BottomNav v-if="!bare && !route.meta.noNav" />
  <Toaster />
  <UpdateBanner />
</template>
