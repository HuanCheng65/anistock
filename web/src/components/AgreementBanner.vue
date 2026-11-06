<script setup lang="ts">
// Documents awaiting acceptance (an existing player after an update, or a new one who left the
// account-opening flow early): trading is closed until they are signed.
import { computed } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { useSession } from '../stores/session'

const session = useSession()
const route = useRoute()
const show = computed(() => !!session.me?.pending_documents.length && route.name !== 'legal')
</script>

<template>
  <div v-if="show" class="ab gutter" role="status">
    <span class="t">签署风险揭示书后才能交易</span>
    <RouterLink :to="{ name: 'welcome', query: { next: route.fullPath } }" class="go">签署</RouterLink>
  </div>
</template>

<style scoped>
.ab {
  display: flex; align-items: baseline; gap: 12px; padding-top: 10px; padding-bottom: 10px;
  border-bottom: 1px solid var(--hair); background: var(--ground); color: var(--ink-2); font-size: 13px; line-height: 1.5;
}
.t { min-width: 0; }
.go { margin-left: auto; flex: none; color: var(--ink); font-weight: 700; text-decoration: underline; text-underline-offset: 3px; }
@media (min-width: 1024px) {
  .ab { max-width: calc(var(--container) + 64px); margin: 0 auto; padding-left: 32px; padding-right: 32px; }
}
</style>
