<script setup lang="ts">
// The work's cover; until it loads (or when there is none) a "book spine": a solid tone with the
// title set vertically.
import { computed, ref, watch } from 'vue'

const props = withDefaults(defineProps<{ id: number; title: string; w: number; h: number; spine?: boolean; size?: number }>(), {
  spine: true,
  size: 17,
})

const TONES = ['#3E5C76', '#7A4E3A', '#5E6B3C', '#7C2F3B', '#4A3E66', '#2F6A5A', '#8A6A2B', '#9C3B2C', '#4B6C8C', '#2E3A2E', '#3D7E8C', '#2D4470', '#B4572E', '#2B2F4A', '#8C3D6B', '#5A4A3A']
const tone = computed(() => TONES[Math.abs(props.id * 2654435761) % TONES.length])
const short = computed(() => spineText(props.title))

const missing = (globalThis as any).__aniexNoCover ?? ((globalThis as any).__aniexNoCover = new Set<number>())
const failed = ref(missing.has(props.id))
const loaded = ref(false)
watch(() => props.id, (id) => {
  failed.value = missing.has(id)
  loaded.value = false
})
function onError() {
  missing.add(props.id)
  failed.value = true
}

function spineText(t: string): string {
  const s = t.replace(/^《|》$/g, '').trim()
  const cut = s.split(/[\s：:～~（(！!·・、,，]/).find((x) => x.length >= 2) ?? s
  return cut
}
</script>

<template>
  <span class="cover" :style="{ width: w + 'px', height: h + 'px', background: tone }">
    <span v-if="spine && !loaded" class="spine serif" :style="{ fontSize: size + 'px', maxHeight: h - 20 + 'px' }" aria-hidden="true">{{ short }}</span>
    <img v-if="!failed" :src="`/api/covers/${id}`" :alt="title" loading="lazy" decoding="async" :class="{ on: loaded }" @load="loaded = true" @error="onError" />
    <slot />
  </span>
</template>

<style scoped>
.cover { position: relative; flex: none; display: block; overflow: hidden; color: var(--spine-ink); }
.spine {
  position: absolute; top: 10px; left: 8px; writing-mode: vertical-rl; font-weight: 900; line-height: 1.15;
  overflow: hidden; letter-spacing: 0.02em; white-space: nowrap;
}
img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; opacity: 0; transition: opacity 0.2s; }
img.on { opacity: 1; }
</style>
