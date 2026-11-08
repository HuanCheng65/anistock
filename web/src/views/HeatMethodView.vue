<script setup lang="ts">
// 指数编制方法: how the AniEx heat index is computed, checked and published (docs/v3-futures.md §3), with the
// constants the server uses (GET /rules heat_index), so any day's value can be checked by hand.
import { computed } from 'vue'
import BackButton from '../components/BackButton.vue'
import { useSession } from '../stores/session'

const session = useSession()
session.loadRules()
const h = computed(() => session.rules?.heat_index ?? null)
const n = (v: number | undefined) => (v == null ? '–' : v.toLocaleString('en-US', { maximumFractionDigits: 0 }))
const w = (v: number | undefined) => (v == null ? '–' : String(v))
</script>

<template>
  <main class="page doc">
    <header class="pagehead back">
      <BackButton />
      <h1>指数编制方法</h1>
    </header>
    <p class="lede">安易热度指数衡量一部作品当天受到的关注。热度期货按它结算，股票页的“热度”显示它的每日值。现行为第 {{ h?.version.replace('v', '') ?? 1 }} 版。</p>

    <section class="sec">
      <h2>当天的热度</h2>
      <ul>
        <li><b>欧美（AniList）</b>：在看人数 ÷ 7、当天新增收藏人数和 trending 值的加权和，按评分加成。与股票营收使用同一个量。</li>
        <li><b>中国（Bangumi）</b>：标记过“在看”“看过”“搁置”或“抛弃”的人数比前一天增加的数量。用户在几种状态之间切换不重复计算。</li>
        <li><b>日本（Annict）</b>：当天新增的观看记录数。同一账号对同一集只计一次。</li>
        <li>每天约 06:00 抓取一次。当天的热度 = 次日抓取的数 − 当天抓取的数，按两次抓取的间隔折算为 24 小时。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>计算</h2>
      <p class="formula num">
        <span>指数 = {{ n(h?.scale) }} ×（</span>
        <span class="in">{{ w(h?.weights.us) }} × 欧美 ÷ {{ n(h?.platform.us) }}</span>
        <span class="in">+ {{ w(h?.weights.cn) }} × 中国 ÷ {{ n(h?.platform.cn) }}</span>
        <span class="in">+ {{ w(h?.weights.jp) }} × 日本 ÷ {{ n(h?.platform.jp) }}）</span>
      </p>
      <ul>
        <li>平台规模是 2025 年 11 月至 2026 年 10 月每天在播作品热度总和的平均，用来把三个平台换算到同一尺度。</li>
        <li>常数使在播作品的中位数为 100。</li>
        <li>作品在某个平台没有条目时，这个平台不计入。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>校验</h2>
      <ul>
        <li>某个平台当天没有抓到数据时，按前 7 天的平均补值。</li>
        <li>当天的数值为负，或超过过去 28 天中位数的 10 倍时，按中位数替换。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>公布与变更</h2>
      <ul>
        <li>每天日结时公布前一个交易日的指数。已公布的值不再修改。</li>
        <li>发现错误时，已公布的值和已结算的合约不追溯，在之后的计算中修正。</li>
        <li>每月复核平台规模和中位数。某个平台的规模连续 3 次偏离超过 20%，或中位数连续 3 个月在 75 至 135 之外时，发布新版本。</li>
        <li>新版本提前 7 天公告，只适用于公告之后开始的合约期。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>回填</h2>
      <ul>
        <li>2025 年 11 月至 2026 年 11 月 4 日的指数由各平台的历史数据回填，图上颜色较浅。</li>
        <li>回填的中国分项来自 Bangumi 每周的存档：一周的增量按同一作品当周每天的 Annict 记录数分配到每一天。</li>
        <li>热度期货只使用实时计算的指数，不使用回填值。</li>
      </ul>
    </section>

    <p class="foot">数据来自 <a href="https://anilist.co" target="_blank" rel="noopener">AniList</a>、<a href="https://bgm.tv" target="_blank" rel="noopener">Bangumi</a>（条目信息 CC BY-SA）、<a href="https://annict.com" target="_blank" rel="noopener">Annict</a> 的公开统计。</p>
  </main>
</template>

<style scoped>
.formula { display: flex; flex-direction: column; font-size: 13px; line-height: 1.8; padding: 10px 12px; background: var(--ground-2); }
.formula .in { padding-left: 2em; }
</style>
