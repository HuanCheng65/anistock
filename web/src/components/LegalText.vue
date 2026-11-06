<script setup lang="ts">
// The body of 用户协议 / 隐私政策 / 风险揭示书 (legal/documents.ts).
import type { Block, LegalDoc } from '../legal/documents'

defineProps<{ doc: LegalDoc }>()
const kind = (b: Block) => ('p' in b ? 'p' : 'ol' in b ? 'ol' : 'table')
</script>

<template>
  <div class="legal">
    <p class="updated">更新日期：{{ doc.updated }}</p>
    <template v-for="(b, i) in doc.intro" :key="'i' + i">
      <p v-if="kind(b) === 'p'" class="para">{{ (b as { p: string }).p }}</p>
    </template>
    <section v-for="s in doc.sections" :key="s.h">
      <h3>{{ s.h }}</h3>
      <template v-for="(b, i) in s.blocks" :key="i">
        <p v-if="'p' in b" class="para">{{ b.p }}</p>
        <ol v-else-if="'ol' in b">
          <li v-for="(x, j) in b.ol" :key="j">{{ x }}</li>
        </ol>
        <div v-else class="wrap">
          <table>
            <thead><tr><th v-for="h in b.table.head" :key="h">{{ h }}</th></tr></thead>
            <tbody><tr v-for="(r, j) in b.table.rows" :key="j"><td v-for="(c, k) in r" :key="k">{{ c }}</td></tr></tbody>
          </table>
        </div>
      </template>
    </section>
    <section v-if="doc.declaration" class="decl">
      <h3>投资者声明</h3>
      <p class="para">{{ doc.declaration }}</p>
    </section>
  </div>
</template>

<style scoped>
.legal { font-size: 14px; line-height: 1.75; color: var(--ink-2); }
.updated { font-size: 12px; color: var(--muted); padding-bottom: 10px; }
.para + .para { margin-top: 8px; }
section { padding-top: 16px; }
h3 { font-family: var(--font-serif); font-size: 16px; font-weight: 900; color: var(--ink); margin-bottom: 6px; }
ol { list-style: decimal; padding-left: 1.4em; display: flex; flex-direction: column; gap: 4px; }
ol + .para, .para + ol { margin-top: 6px; }
.wrap { margin: 4px 0 8px; }
table { width: 100%; border-collapse: collapse; table-layout: fixed; font-size: 13px; line-height: 1.6; }
th:nth-child(1) { width: 36%; }
th:nth-child(3) { width: 22%; }
th { text-align: left; font-weight: 400; font-size: 12px; color: var(--muted); padding: 6px 12px 6px 0; border-bottom: 1px solid var(--hair); }
td { vertical-align: top; padding: 8px 12px 8px 0; border-bottom: 1px solid var(--hair-2); overflow-wrap: anywhere; }
td:last-child, th:last-child { padding-right: 0; }
.decl .para { color: var(--ink); }
</style>
