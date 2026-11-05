<script setup lang="ts">
import { ICON } from './icons'
withDefaults(defineProps<{ name: keyof typeof ICON; size?: number; fill?: boolean }>(), { size: 22 })
</script>

<template>
  <svg :width="size" :height="size" viewBox="0 0 24 24" aria-hidden="true" class="icon">
    <path :d="ICON[name]" :fill="fill ? 'currentColor' : 'none'" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
</template>

<style scoped>
.icon { display: block; flex: none; }
</style>
