<script setup lang="ts">
// Underlined text tabs (the mockups' segmented tabs).
defineProps<{ items: { key: string; label: string }[]; label: string; size?: number }>()
const model = defineModel<string>({ required: true })
</script>

<template>
  <div class="tabs" role="tablist" :aria-label="label">
    <button v-for="t in items" :key="t.key" type="button" role="tab" :aria-selected="model === t.key"
            :style="size ? { fontSize: size + 'px' } : undefined" @click="model = t.key">{{ t.label }}</button>
    <slot />
  </div>
</template>

<style scoped>
.tabs { display: flex; gap: 22px; border-bottom: 1px solid var(--hair); align-items: flex-end; }
button {
  border: 0; background: transparent; padding: 6px 0 10px; font-size: 15px; color: var(--muted);
  border-bottom: 2px solid transparent; margin-bottom: -1px; white-space: nowrap;
}
button[aria-selected="true"] { color: var(--ink); font-weight: 700; border-bottom-color: var(--ink); }
</style>
