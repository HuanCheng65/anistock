<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { PHASE_LABEL } from '../format'
import { useSession } from '../stores/session'
const props = defineProps<{ seconds?: boolean }>()
const session = useSession()
const now = ref(new Date())
const timer = window.setInterval(() => (now.value = new Date()), 1000)
onBeforeUnmount(() => window.clearInterval(timer))
const phase = computed(() => session.status?.phase ?? 'closed')
const pad = (n: number) => String(n).padStart(2, '0')
// game clock = Asia/Shanghai
const clock = computed(() => {
  const t = new Date(now.value.getTime() + (now.value.getTimezoneOffset() + 480) * 60000)
  return `${pad(t.getHours())}:${pad(t.getMinutes())}` + (props.seconds ? `:${pad(t.getSeconds())}` : '')
})
</script>

<template>
  <span class="clock" v-if="session.status">
    <span class="dot" :class="phase"></span>
    <span>{{ PHASE_LABEL[phase] }}</span>
    <span class="num">{{ clock }}</span>
  </span>
</template>

<style scoped>
.clock { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--ink-2); white-space: nowrap; }
.dot { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); }
.dot.continuous { background: var(--up); }
.dot.auction { background: var(--mk-cn); }
</style>
