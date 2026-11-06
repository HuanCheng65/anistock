<script setup lang="ts">
// Underlined text tabs (the mockups' segmented tabs). A tablist with a roving tab stop: ←/→ (and Home/End)
// move between tabs and select them; the underline slides to the selected tab. With `ids`, each tab names
// its panel (`${ids}-panel-${key}`) and is named by it (`${ids}-tab-${key}`).
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
const props = defineProps<{ items: { key: string; label: string }[]; label: string; size?: number; ids?: string }>()
const model = defineModel<string>({ required: true })

const root = ref<HTMLElement | null>(null)
const bar = ref({ x: 0, w: 0, ready: false, animate: false })

function tabEl(key: string): HTMLElement | null {
  return root.value?.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`) ?? null
}
function place(animate: boolean) {
  const el = tabEl(model.value)
  if (!el) {
    bar.value = { ...bar.value, ready: false }
    return
  }
  bar.value = { x: el.offsetLeft, w: el.offsetWidth, ready: true, animate }
}
function select(key: string, focus = false) {
  model.value = key
  if (focus) nextTick(() => tabEl(key)?.focus())
}
function onKey(e: KeyboardEvent, i: number) {
  const n = props.items.length
  let j = -1
  if (e.key === 'ArrowRight') j = (i + 1) % n
  else if (e.key === 'ArrowLeft') j = (i - 1 + n) % n
  else if (e.key === 'Home') j = 0
  else if (e.key === 'End') j = n - 1
  if (j < 0) return
  e.preventDefault()
  select(props.items[j].key, true)
}

watch(model, () => nextTick(() => place(true)))
watch(() => props.items.map((t) => t.key + t.label).join(), () => nextTick(() => place(false)))
let ro: ResizeObserver | null = null
onMounted(() => {
  place(false)
  // fonts and the container's width settle after mount
  ro = new ResizeObserver(() => place(false))
  if (root.value) ro.observe(root.value)
  document.fonts?.ready.then(() => place(false)).catch(() => {})
})
onBeforeUnmount(() => ro?.disconnect())
</script>

<template>
  <div ref="root" class="tabs" role="tablist" :aria-label="label">
    <button v-for="(t, i) in items" :id="ids ? `${ids}-tab-${t.key}` : undefined" :key="t.key" type="button" role="tab"
            :data-key="t.key" :aria-selected="model === t.key" :aria-controls="ids ? `${ids}-panel-${t.key}` : undefined"
            :tabindex="model === t.key ? 0 : -1" :style="size ? { fontSize: size + 'px' } : undefined"
            @click="select(t.key)" @keydown="onKey($event, i)">{{ t.label }}</button>
    <slot />
    <span v-if="bar.ready" class="ink" :class="{ animate: bar.animate }" aria-hidden="true"
          :style="{ transform: `translateX(${bar.x}px)`, width: bar.w + 'px' }"></span>
  </div>
</template>

<style scoped>
.tabs { position: relative; display: flex; gap: 22px; border-bottom: 1px solid var(--hair); align-items: flex-end; }
button {
  border: 0; background: transparent; padding: 6px 0 10px; font-size: 15px; color: var(--muted);
  border-bottom: 2px solid transparent; margin-bottom: -1px; white-space: nowrap;
  transition: color var(--dur-1) var(--ease-out);
}
button[aria-selected="true"] { color: var(--ink); font-weight: 700; }
@media (hover: hover) { button:hover { color: var(--ink); } }
button:focus-visible { outline-offset: -2px; }
.ink { position: absolute; left: 0; bottom: -1px; height: 2px; background: var(--ink); pointer-events: none; }
.ink.animate { transition: transform var(--dur-2) var(--ease-out), width var(--dur-2) var(--ease-out); }
@media (prefers-reduced-motion: reduce) { .ink.animate, button { transition: none; } }
</style>
