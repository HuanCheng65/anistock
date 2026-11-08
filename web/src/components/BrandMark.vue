<script setup lang="ts">
defineProps<{ size?: number }>()
</script>

<template>
  <svg :width="size ?? 28" :height="size ?? 28" viewBox="0 0 64 64" aria-hidden="true" class="brand-mark">
    <g transform="translate(32 31.5) scale(0.991) translate(-32.44 -32.91)">
      <path d="M36 1 C37.12 9.4 39 11.65 46 13 C39 14.35 37.12 16.6 36 25 C34.88 16.6 33 14.35 26 13 C33 11.65 34.88 9.4 36 1 Z" fill="currentColor" />
      <path d="M27 25H35L21 58H12Z" fill="currentColor" />
      <path d="M37 25H45L53 58H44Z" style="fill: var(--brand)" />
    </g>
  </svg>
</template>

<style scoped>
.brand-mark { flex: none; display: block; color: var(--ink); }
</style>
