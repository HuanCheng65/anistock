<script setup lang="ts">
// 融资融券: a margin call, a forced liquidation or a default, on every page until it is resolved.
import { computed } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import Icon from './Icon.vue'
import { useSession } from '../stores/session'

const session = useSession()
const route = useRoute()
const text = computed(() => {
  switch (session.me?.credit_status) {
    case 'call':
      return '维持担保比例低于 130%。下次日结前需回到 130% 以上，否则强制平仓'
    case 'liquidating':
      return '强制平仓中：卖出持仓、买券还券，直到维持担保比例回到 150%'
    case 'defaulted':
      return '已穿仓：负债超过资产。可以申请破产重整'
    default:
      return ''
  }
})
</script>

<template>
  <div v-if="text" class="mb gutter" role="alert">
    <Icon name="warning" :size="16" class="w" />
    <span class="t">{{ text }}</span>
    <RouterLink v-if="route.name !== 'me'" to="/me" class="go">查看</RouterLink>
  </div>
</template>

<style scoped>
.mb {
  display: flex; align-items: baseline; gap: 8px; padding-top: 10px; padding-bottom: 10px;
  border-bottom: 1px solid var(--hair); background: var(--ground); color: var(--danger); font-size: 13px; line-height: 1.5;
}
.w { align-self: center; }
.t { min-width: 0; }
.go { margin-left: auto; padding-left: 4px; flex: none; color: var(--danger); text-decoration: underline; text-underline-offset: 3px; }
@media (min-width: 1024px) {
  .mb { max-width: calc(var(--container) + 64px); margin: 0 auto; padding-left: 32px; padding-right: 32px; }
}
</style>
