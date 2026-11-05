<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from 'vue'
import { RouterView, useRoute } from 'vue-router'
import BottomNav from './components/BottomNav.vue'
import DeskHeader from './components/DeskHeader.vue'
import { useMarket } from './stores/market'
import { usePrefs } from './stores/prefs'
import { useSession } from './stores/session'

const route = useRoute()
const session = useSession()
const market = useMarket()
usePrefs() // applies the saved theme
const bare = computed(() => !!route.meta.bare)

let timer: number | undefined
function tick() {
  if (document.hidden) return
  session.loadStatus()
  market.refreshIndices()
}
onMounted(() => {
  tick()
  timer = window.setInterval(tick, 30000)
})
onBeforeUnmount(() => window.clearInterval(timer))
</script>

<template>
  <DeskHeader v-if="!bare" />
  <RouterView v-slot="{ Component }">
    <component :is="Component" :key="route.path" />
  </RouterView>
  <BottomNav v-if="!bare && !route.meta.noNav" />
</template>
