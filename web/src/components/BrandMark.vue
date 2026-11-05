<script setup lang="ts">
defineProps<{ size?: number }>()
</script>

<template>
  <svg :width="size ?? 28" :height="size ?? 28" viewBox="0 0 64 64" aria-hidden="true" class="brand-mark">
    <path d="M32 2C32.9 7.6 34.4 9.1 40 10C34.4 10.9 32.9 12.4 32 18C31.1 12.4 29.6 10.9 24 10C29.6 9.1 31.1 7.6 32 2Z" fill="currentColor" />
    <path d="M27 21H35L21 58H12Z" fill="currentColor" />
    <path d="M37 21H45L53 58H44Z" fill="var(--seal)" />
  </svg>
</template>

<style scoped>
.brand-mark { flex: none; display: block; color: var(--ink); }
</style>
