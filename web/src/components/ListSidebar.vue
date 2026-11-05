<script setup lang="ts">
// Detail page list: 自选 / 持仓 / the list the stock was opened from. Current stock marked with a red bar.
import { nextTick, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import type { MarketStock } from '../api'
import { money, pct, dir } from '../format'
import Icon from './Icon.vue'
import Tabs from './Tabs.vue'

const props = defineProps<{ rows: MarketStock[]; tabs: { key: string; label: string }[]; current: number; collapsible?: boolean }>()
const tab = defineModel<string>('tab', { required: true })
defineEmits<{ collapse: []; pick: [] }>()
const list = ref<HTMLElement>()
watch(() => props.current, async () => {
  await nextTick()
  list.value?.querySelector('.cur')?.scrollIntoView({ block: 'nearest' })
}, { immediate: true })
</script>

<template>
  <aside class="side" aria-label="列表">
    <div class="head">
      <Tabs v-model="tab" :items="tabs" label="列表" :size="13" class="tabs" />
      <button v-if="collapsible" type="button" class="cbtn" aria-label="收起列表" @click="$emit('collapse')"><Icon name="back" :size="16" /></button>
    </div>
    <div ref="list" class="rows">
      <RouterLink v-for="s in rows" :key="s.id" :to="`/stocks/${s.id}`" replace class="w" :class="{ cur: s.id === current }" :aria-current="s.id === current ? 'true' : undefined" @click="$emit('pick')">
        <span class="clamp2 t">{{ s.title }}</span>
        <span v-flash="s.price" class="num px">{{ money(s.price) }}</span>
        <span class="num code">{{ s.code }}</span>
        <span v-flash="s.change" class="num chg" :class="dir(s.change)">{{ pct(s.change) }}</span>
      </RouterLink>
      <p v-if="!rows.length" class="none">{{ tab === 'watch' ? '暂无自选' : tab === 'hold' ? '暂无持仓' : '暂无股票' }}</p>
    </div>
    <div class="num foot">↑ ↓ 切换</div>
  </aside>
</template>

<style scoped>
.side { display: flex; flex-direction: column; min-width: 0; min-height: 0; }
.head { display: flex; align-items: flex-end; border-bottom: 1px solid var(--hair); padding: 0 0 0 14px; }
.tabs { border-bottom: 0; gap: 16px; padding-top: 10px; flex: 1; min-width: 0; overflow: hidden; }
.cbtn { width: 26px; height: 26px; margin: 0 6px 6px; border: 0; background: transparent; padding: 0; color: var(--muted); display: grid; place-items: center; }
.rows { flex: 1; overflow: auto; min-height: 0; }
.w { display: grid; grid-template-columns: minmax(0, 1fr) auto; column-gap: 10px; row-gap: 2px; padding: 9px 14px; border-bottom: 1px solid var(--hair-2); color: var(--ink); }
.w.cur { background: var(--ground-2); box-shadow: inset 3px 0 0 var(--ink); }
.w.cur .t { font-weight: 700; }
.t { font-size: 13px; line-height: 1.4; }
.px { font-size: 13px; text-align: right; }
.code { font-size: 11px; color: var(--muted); }
.chg { font-size: 12px; text-align: right; align-self: end; }
.none { padding: 16px 14px; font-size: 13px; color: var(--muted); }
.foot { padding: 10px 14px; font-size: 12px; color: var(--muted); border-top: 1px solid var(--hair-2); }
</style>
