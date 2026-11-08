<script setup lang="ts">
// The AniEx wordmark and lockups, from brand/build.py. Ink follows currentColor, the red leg --brand.
import { computed } from 'vue'
import { lockups } from './brand/lockups'

const props = defineProps<{ kind?: keyof typeof lockups; height: number }>()
const l = computed(() => lockups[props.kind ?? 'horizontal'])
</script>

<template>
  <svg :viewBox="l.viewBox" :height="height" :width="(height * l.width) / l.height" role="img" :aria-label="l.label" class="brand-lockup" v-html="l.body" />
</template>

<style scoped>
.brand-lockup { display: block; flex: none; color: var(--ink); }
</style>
