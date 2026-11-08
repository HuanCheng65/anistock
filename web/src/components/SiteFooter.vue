<script setup lang="ts">
// The site footer on reading pages (规则 and its sub-pages, 新手指南, the legal documents), the bottom of 我的, the
// signed-out market page and the login page. Not on the trading pages: the bottom nav and the terminal layout
// already fill the screen there. `noNotice` leaves out the virtual-asset line where the page already says it.
import { RouterLink } from 'vue-router'

defineProps<{ noNotice?: boolean }>()
</script>

<template>
  <footer class="sitefoot">
    <nav class="links" aria-label="站点">
      <RouterLink to="/rules">规则</RouterLink>
      <RouterLink to="/guide">新手指南</RouterLink>
      <RouterLink to="/rules/api">开发者说明</RouterLink>
      <RouterLink to="/legal/terms">用户协议</RouterLink>
      <RouterLink to="/legal/privacy">隐私政策</RouterLink>
    </nav>
    <p v-if="!noNotice">星辉是安易内的虚拟资产，与真实货币无关，不能充值或提现。</p>
    <p>作品数据来自 <a href="https://anilist.co" target="_blank" rel="noopener">AniList</a>、<a href="https://bgm.tv" target="_blank" rel="noopener">Bangumi</a>（条目信息 CC BY-SA）、<a href="https://annict.com" target="_blank" rel="noopener">Annict</a> 的公开统计。</p>
    <p class="num">© 2026 Astral Studio</p>
  </footer>
</template>

<style scoped>
.sitefoot { margin-top: 24px; padding: 16px var(--gutter) 8px; border-top: 1px solid var(--hair); font-size: 12px; line-height: 1.7; color: var(--muted); }
/* inside a .container that already sets the side margins */
.sitefoot.flush { padding-left: 0; padding-right: 0; }
.links { display: flex; flex-wrap: wrap; column-gap: 16px; row-gap: 4px; margin-bottom: 8px; }
.links a { color: var(--ink-2); }
.sitefoot p a { color: inherit; text-decoration: underline; text-decoration-color: var(--hair); text-underline-offset: 2px; }
@media (hover: hover) {
  .links a:hover, .sitefoot p a:hover { color: var(--ink); }
}
</style>
