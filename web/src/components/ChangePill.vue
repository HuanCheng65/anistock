<script setup lang="ts">
import { computed } from 'vue'
import { dir, pct } from '../format'
const props = defineProps<{ value: number | null | undefined }>()
const d = computed(() => dir(props.value))
</script>

<template>
  <span class="pill num" :class="d">{{ pct(value ?? 0) }}</span>
</template>

<style scoped>
.pill { display: inline-block; min-width: 72px; text-align: center; font-size: 13px; padding: 2px 6px; color: var(--on-pill); background: var(--pill-flat); }
.pill.up { background: var(--pill-up); }
.pill.down { background: var(--pill-down); }
</style>
