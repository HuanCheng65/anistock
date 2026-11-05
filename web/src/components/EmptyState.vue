<script setup lang="ts">
// 空状态 / 错误: what is the case, and the next step.
import { RouterLink, type RouteLocationRaw } from 'vue-router'
defineProps<{ text: string; action?: string; to?: RouteLocationRaw; tone?: 'error' }>()
defineEmits<{ act: [] }>()
</script>

<template>
  <div class="empty" :class="tone" :role="tone === 'error' ? 'alert' : undefined">
    <p>{{ text }}</p>
    <RouterLink v-if="action && to" :to="to" class="btn ghost small">{{ action }}</RouterLink>
    <button v-else-if="action" type="button" class="btn ghost small" @click="$emit('act')">{{ action }}</button>
  </div>
</template>

<style scoped>
.empty { padding: 28px var(--gutter); display: flex; flex-direction: column; align-items: center; gap: 12px; text-align: center; color: var(--muted); font-size: 14px; }
.empty.error { color: var(--up); }
</style>
