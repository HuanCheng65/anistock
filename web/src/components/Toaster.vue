<script setup lang="ts">
import { useToast } from '../stores/toast'
const toast = useToast()
</script>

<template>
  <div class="toaster" role="status" aria-live="polite">
    <TransitionGroup name="toast">
      <button v-for="t in toast.items" :key="t.id" type="button" class="toast term" :class="t.tone" @click="toast.dismiss(t.id)">
        <span class="txt num">{{ t.text }}</span>
        <span v-if="t.sub" class="sub clamp1">{{ t.sub }}</span>
      </button>
    </TransitionGroup>
  </div>
</template>

<style scoped>
.toaster {
  position: fixed; z-index: 60; left: 50%; transform: translateX(-50%);
  bottom: calc(var(--nav-h) + 16px + env(safe-area-inset-bottom, 0px));
  display: flex; flex-direction: column; align-items: center; gap: 8px; pointer-events: none; width: max-content; max-width: calc(100vw - 32px);
}
@media (min-width: 1024px) { .toaster { bottom: 28px; } }
.toast {
  pointer-events: auto; display: flex; flex-direction: column; align-items: flex-start; gap: 2px; text-align: left;
  border: 0; padding: 10px 16px 10px 13px; min-width: 220px; max-width: 100%; border-left: 3px solid var(--t-ink-2);
}
.toast.buy { border-left-color: var(--t-up); }
.toast.sell { border-left-color: var(--t-down); }
.txt { font-size: 14px; font-weight: 600; color: var(--t-ink); }
.sub { font-size: 12px; color: var(--t-ink-2); max-width: 22em; }
.toast-enter-active, .toast-leave-active { transition: opacity 0.2s ease, transform 0.2s ease; }
.toast-enter-from, .toast-leave-to { opacity: 0; transform: translateY(8px); }
@media (prefers-reduced-motion: reduce) {
  .toast-enter-active, .toast-leave-active { transition: none; }
}
</style>
