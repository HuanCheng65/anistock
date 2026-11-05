<script setup lang="ts">
// Market status and game clock. The dot pulses while prices arrive live (WebSocket open, market open).
// Neutral colours on purpose: red and green mean price direction only.
import { computed, onBeforeUnmount, ref } from 'vue'
import { liveState } from '../live'
import { PHASE_LABEL } from '../format'
import { useSession } from '../stores/session'
const props = defineProps<{ seconds?: boolean }>()
const session = useSession()
const now = ref(new Date())
const timer = window.setInterval(() => (now.value = new Date()), 1000)
onBeforeUnmount(() => window.clearInterval(timer))
const phase = computed(() => session.status?.phase ?? 'closed')
const state = computed(() => {
  if (liveState.value === 'down') return { cls: 'down', text: '连接中断' }
  if (phase.value === 'closed') return { cls: 'closed', text: PHASE_LABEL.closed }
  if (liveState.value !== 'open') return { cls: 'wait', text: PHASE_LABEL[phase.value] }
  return { cls: 'live', text: phase.value === 'auction' ? PHASE_LABEL.auction : '实时' }
})
const pad = (n: number) => String(n).padStart(2, '0')
// game clock = Asia/Shanghai
const clock = computed(() => {
  const t = new Date(now.value.getTime() + (now.value.getTimezoneOffset() + 480) * 60000)
  return `${pad(t.getHours())}:${pad(t.getMinutes())}` + (props.seconds ? `:${pad(t.getSeconds())}` : '')
})
</script>

<template>
  <span class="clock" :class="state.cls" v-if="session.status">
    <span class="dot" aria-hidden="true"></span>
    <span class="st">{{ state.text }}</span>
    <span class="num">{{ clock }}</span>
  </span>
</template>

<style scoped>
.clock { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--ink-2); white-space: nowrap; }
.dot { position: relative; width: 7px; height: 7px; border-radius: 50%; background: var(--muted); }
.wait .dot { background: var(--ink-2); }
.live .dot { background: var(--ink); }
.live .st { color: var(--ink); }
.live .dot::after {
  content: ''; position: absolute; inset: 0; border-radius: 50%; background: var(--ink);
  animation: ping 2.4s ease-out infinite;
}
@keyframes ping { 0% { transform: scale(1); opacity: 0.45; } 70%, 100% { transform: scale(2.6); opacity: 0; } }
.down .dot { background: transparent; box-shadow: inset 0 0 0 1px var(--muted); }
.down .st { color: var(--muted); }
@media (prefers-reduced-motion: reduce) { .live .dot::after { animation: none; opacity: 0; } }
</style>
