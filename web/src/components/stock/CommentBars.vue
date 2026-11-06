<script setup lang="ts">
// Per-episode discussion (Bangumi comments per episode), one bar each; pointing at, focusing or tapping an
// episode shows its count, otherwise the latest. Bars use the China market's colour: this is the public data
// that market's revenue is built from.
import { computed, ref } from 'vue'
import type { Work } from '../../api'
import { big } from '../../format'
const props = withDefaults(defineProps<{ items: Work['discussion']; h?: number }>(), { h: 48 })
const max = computed(() => Math.max(1, ...props.items.map((x) => x.comments)))
const picked = ref<number | null>(null)
const hovering = ref(false)
const cur = computed(() => picked.value ?? props.items.length - 1)
const x = computed(() => props.items[cur.value] ?? null)
const root = ref<HTMLElement | null>(null)
function onKey(e: KeyboardEvent) {
  const n = props.items.length
  let j = -1
  if (e.key === 'ArrowRight') j = Math.min(n - 1, cur.value + 1)
  else if (e.key === 'ArrowLeft') j = Math.max(0, cur.value - 1)
  else if (e.key === 'Home') j = 0
  else if (e.key === 'End') j = n - 1
  if (j < 0) return
  e.preventDefault()
  picked.value = j
  root.value?.querySelectorAll<HTMLElement>('.b')[j]?.focus()
}
function leave() {
  hovering.value = false
  if (!root.value?.contains(document.activeElement)) picked.value = null
}
</script>

<template>
  <div class="cb">
    <div class="read" aria-live="polite">
      <span v-if="x">第 {{ x.episode }} 集</span>
      <span v-if="x" class="num v">{{ big(x.comments) }} 条吐槽</span>
    </div>
    <div ref="root" class="bars" :class="{ dim: picked != null || hovering }" :style="{ height: h + 'px' }" role="group" aria-label="单集讨论"
         @pointerleave="leave" @keydown="onKey">
      <button v-for="(it, i) in items" :key="it.episode" type="button" class="b" :class="{ on: i === cur }" :tabindex="i === cur ? 0 : -1"
              :aria-label="`第 ${it.episode} 集，${it.comments} 条吐槽`" :aria-pressed="i === cur"
              @pointerenter="hovering = true; picked = i" @focus="picked = i" @click="picked = i">
        <span :style="{ height: Math.max(2, (it.comments / max) * h) + 'px' }"></span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.cb { display: flex; flex-direction: column; gap: 6px; }
.read { display: flex; justify-content: space-between; gap: 12px; font-size: 12px; color: var(--muted); min-height: 18px; }
.read .v { color: var(--ink); font-size: 13px; }
.bars { display: flex; align-items: flex-end; gap: 3px; touch-action: pan-y; }
.b { flex: 1; max-width: 28px; min-width: 0; height: 100%; display: flex; align-items: flex-end; border: 0; padding: 0; background: transparent; cursor: pointer; transition: opacity var(--dur-1) var(--ease-out); }
.b span { display: block; width: 100%; background: var(--mk-cn); transition: height var(--dur-2) var(--ease-out); }
.dim .b:not(.on) { opacity: 0.4; }
.b:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
@media (prefers-reduced-motion: reduce) { .b, .b span { transition: none; } }
</style>
