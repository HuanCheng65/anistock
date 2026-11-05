<script setup lang="ts">
import { computed } from 'vue'
import BackButton from '../components/BackButton.vue'
import { taxBrackets } from '../format'
import { useSession } from '../stores/session'

const session = useSession()
const r = computed(() => session.status?.rules)
const st = computed(() => session.status)
const p = (v: number | undefined, d = 2) => (v == null ? '–' : (v * 100).toFixed(d) + '%')
const taxText = computed(() => taxBrackets(r.value?.dividend_tax).map((x) => `${x.k} ${x.v}`).join('，'))
</script>

<template>
  <main class="page doc">
    <header class="pagehead back">
      <BackButton />
      <h1>规则</h1>
    </header>
    <p class="lede">交易规则参考港股和美股。为便于上手做过的简化标为“简化”。</p>

    <section class="sec">
      <h2>公司与营收</h2>
      <ul>
        <li>每部番是一家上市公司，总股本 10,000 股。25% 公开发行，75% 由“制作委员会”（真实世界的版权方）持有。</li>
        <li>每天的外部需求按三个市场的热度份额分给各公司：中国（Bangumi 每集讨论）50%、欧美（AniList 活跃度和新增收藏）30%、日本（Annict 每集看过人数）20%。全市场越热，总需求越大。</li>
        <li>营收的一半是运营成本（发行、制作、宣传），付给外部；剩下的交 25% 企业所得税。</li>
        <li>税后利润的 90% 在每周日作为分红发给股东，其余留在公司账上，就是“每股净资产”。</li>
        <li>制作委员会分到的红利流出游戏，相当于真实世界的利润汇回版权方。</li>
        <li>番剧完结后营收会迅速下降（完结后第一周约为平时的四成）。完结、营收很低并持续 8 周的公司退市，账上现金按股数返还给股东。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>交易时间</h2>
      <ul>
        <li>{{ st?.auction_start ?? '08:45' }}–{{ st?.open ?? '09:00' }} 开盘集合竞价；{{ st?.open ?? '09:00' }}–{{ st?.close ?? '23:00' }} 连续交易；每天 06:30 日结。<span class="simp">简化</span>真实交易所每天只开几个小时，社团成员晚上才有空。</li>
        <li>没成交的委托在每天日结时自动撤销（委托当日有效）。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>开盘集合竞价</h2>
      <ul>
        <li>竞价时段只收限价单，先不成交。页面会显示当前的参考开盘价和可成交股数。</li>
        <li>{{ st?.open ?? '09:00' }} 时，按能成交最多股数的那一个价格，把所有能成交的买卖单一次性撮合，这就是当天的开盘价，然后进入连续交易。没有可成交的单子时，以昨收为开盘价。</li>
        <li>深夜播出的剧集无论好坏，第二天所有人在同一时间按同一价格反应。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>连续交易</h2>
      <ul>
        <li>价格优先、时间优先，按先挂单一方的价格成交。</li>
        <li>限价单：指定价格，能成交的立即成交，剩下的挂着等，可以撤单。市价单：从盘口最优价开始成交，成交不了的部分立即撤销。</li>
        <li>最小 1 股。<span class="simp">简化</span>A 股是 100 股一手。</li>
        <li>T+0：当天买的当天就能卖（港股、美股也是 T+0）。</li>
        <li>不能卖空，不能加杠杆。</li>
        <li>费用：买卖双方各付佣金 {{ p(r?.commission, 3) }}，卖方另付印花税 {{ p(r?.stamp_duty) }}。<span class="simp">简化</span>没有每笔最低佣金。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>没有涨跌停，有冷静期</h2>
      <ul>
        <li>参考港股的市场波动调节机制：价格可以一天之内大涨大跌，剧集的好坏当天就能反映到股价上。</li>
        <li>当天第一次出现成交价比 {{ r?.vcm_window_min ?? 5 }} 分钟前的价格偏离超过 {{ p(r?.vcm_threshold, 0) }} 时，这笔不成交，股票进入 {{ r?.vcm_cool_min ?? 5 }} 分钟冷静期，期间只能在参考价 ±{{ p(r?.vcm_threshold, 0) }} 内成交，区间外的单子挂着等。</li>
        <li>之后以当天开盘价为锚：偏离开盘价超过 20%、30%……时依次再次触发冷静期。<span class="simp">调整</span>港股是每天一次；逐级触发是为了防止小市场里被大户一路推高。</li>
        <li>极端价格保护：限价单价格超过昨收 20 倍会被拒绝。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>新股认购</h2>
      <ul>
        <li>新番在首播前约 3 周上市，发行价是按开播前热度估出来的价值打九折。</li>
        <li>上市前 3 天开放认购：按发行价冻结资金（股数 × 发行价），截止前可以改数量或撤回。</li>
        <li>认购截止当天日结时分配。公开发售的基础是流通盘的 50%；超额认购 15 倍以上回拨到 70%，50 倍以上 80%，100 倍以上 90%。</li>
        <li>认购超过公开发售量时按比例分配，零头抽签；没分到的资金退回。没卖出去的股份由制作委员会留下。</li>
        <li>分配后次日上市交易，参考港股的公开发售与回拨机制。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>账户、利息与税</h2>
      <ul>
        <li>注册需要社团邀请码。初始资金 {{ (r?.initial_cash ?? 100000).toLocaleString('zh-CN') }} 币。</li>
        <li>现金每天按存款利率计息，存款利率 = 政策利率 − 1%。</li>
        <li>股息红利税按持股时间计算，先买的先算：{{ taxText }}。</li>
        <li>每周日，财政会把本周税收的 25% 平均发给所有玩家（财政补贴）。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>央行</h2>
      <ul>
        <li>每周日，央行看过去 28 天货币供应量（M2）的年化增速：高于 20% 就加息，低于就降息，每次最多 0.25%。</li>
        <li>利率影响存款利息，也影响股票估值：利率越高，未来分红越不值钱，长篇番受影响最大。</li>
      </ul>
    </section>

    <section class="sec">
      <h2>市场里的 NPC</h2>
      <ul>
        <li>做市商：每支股票一个，在自己估计的价格两边挂单。它的资金和持股有限，卖得越多报价越高，也会看错、会亏钱。</li>
        <li>制作委员会：股价远高于它认为的价值时，会少量减持。</li>
        <li>散户：偶尔随机买卖一点。</li>
        <li>系统内部的估值不对玩家公开。你能看到的是营收、分红、集数、观众数这些原始数据。</li>
      </ul>
    </section>

    <p class="foot">游戏币与真实货币无关，不能充值或提现。作品数据来自 AniList、Bangumi、Annict 的公开统计。</p>
  </main>
</template>

<style scoped>
.simp { display: inline-block; margin-left: 6px; font-size: 11px; padding: 0 5px; border: 1px solid var(--brand); color: var(--brand); line-height: 1.5; }
</style>
