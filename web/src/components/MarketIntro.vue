<script setup lang="ts">
// 市场 for a visitor: what AniEx is, in one line, with the way in.
import { RouterLink } from 'vue-router'
</script>

<template>
  <section class="intro" aria-labelledby="intro-h">
    <h1 id="intro-h">动画作品交易所</h1>
    <p class="lede">买卖看好的作品，和其他玩家比较收益。</p>
    <div class="acts">
      <RouterLink :to="{ name: 'login', query: { register: '1' } }" class="btn">注册</RouterLink>
      <RouterLink to="/guide" class="btn ghost">新手指南</RouterLink>
    </div>
    <p class="note">注册获得 100,000 星辉。星辉是安易内的虚拟资产，与真实货币无关，不能充值或提现。</p>
  </section>
</template>

<style scoped>
.intro { padding: 22px 0 20px; border-bottom: 1px solid var(--rule); }
h1 { font-family: var(--font-serif); font-size: 26px; font-weight: 900; line-height: 1.3; }
.lede { font-size: 15px; line-height: 1.7; color: var(--ink-2); margin-top: 8px; }
.acts { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 16px; }
.note { font-size: 12px; line-height: 1.6; color: var(--muted); margin-top: 12px; }
@media (min-width: 1024px) {
  .intro { display: grid; grid-template-columns: minmax(0, 1fr) auto; column-gap: 32px; align-items: end; padding: 32px 0 24px; }
  h1 { font-size: 32px; }
  .lede, .note { grid-column: 1; }
  .acts { grid-column: 2; grid-row: 1 / span 2; display: flex; margin-top: 0; }
  .acts .btn { min-width: 120px; }
}
</style>
