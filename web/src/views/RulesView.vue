<script setup lang="ts">
// 规则: the reference. Four parts, each section with a fixed anchor (/rules#vcm) that other pages link to; a
// contents list beside it on desktop and at the top on phones, marking the section being read. Numbers the server
// publishes (GET /rules) are read from it.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import BackButton from '../components/BackButton.vue'
import { money, sessionText, taxBrackets } from '../format'
import { useSession } from '../stores/session'

const session = useSession()
session.loadRules()
const r = computed(() => session.rules)
const st = computed(() => session.rules?.session)
const p = (v: number | undefined, d = 2) => (v == null ? '–' : `${+(v * 100).toFixed(d)}%`)
const taxes = computed(() => taxBrackets(r.value?.dividend_tax))
const vcmBands = computed(() => (r.value?.vcm.later_bands ?? [0.2, 0.3]).map((b) => p(b, 0)).join('、'))
const clawback = computed(() => {
  const ipo = r.value?.ipo
  const base = ipo?.public_base ?? 0.5
  const bands = ipo?.clawback ?? [{ oversubscribed: 15, public_share: 0.7 }, { oversubscribed: 50, public_share: 0.8 }, { oversubscribed: 100, public_share: 0.9 }]
  return [{ k: `${bands[0]?.oversubscribed ?? 15} 倍以下`, v: p(base, 0) }, ...bands.map((b) => ({ k: `${b.oversubscribed} 倍以上`, v: p(b.public_share, 0) }))]
})

type Item = readonly [id: string, title: string]
const PARTS: readonly { label: string; items: readonly Item[] }[] = [
  { label: '基础', items: [['company', '公司与营收'], ['hours', '交易时间'], ['orders', '委托与成交'], ['fees', '费用与税'], ['vcm', '冷静期']] },
  { label: '新股与公司行为', items: [['ipo', '新股认购'], ['schedule', '排期变化'], ['corporate', '拆股、配股与回购'], ['delist', '退市']] },
  { label: '进阶', items: [['margin', '融资融券'], ['futures', '热度期货'], ['funds', '指数基金']] },
  { label: '账户与市场环境', items: [['account', '账户与利息'], ['central-bank', '央行'], ['indices', '指数'], ['npc', '市场里的 NPC']] },
]
const ALL: Item[] = PARTS.flatMap((x) => x.items)
const title = (id: string) => ALL.find(([k]) => k === id)?.[1] ?? ''

// the section being read: the last one whose heading has passed the top quarter of the window
const current = ref<string>(ALL[0][0])
const open = ref(false)
let frame = 0
function spy() {
  cancelAnimationFrame(frame)
  frame = requestAnimationFrame(() => {
    let cur: string = ALL[0][0]
    for (const [id] of ALL) {
      const el = document.getElementById(id)
      if (el && el.getBoundingClientRect().top < window.innerHeight * 0.25) cur = id
    }
    current.value = cur
  })
}
function go(id: string) {
  open.value = false
  history.replaceState(history.state, '', `#${id}`)
  document.getElementById(id)?.scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' })
}
onMounted(() => {
  window.addEventListener('scroll', spy, { passive: true })
  spy()
})
onBeforeUnmount(() => window.removeEventListener('scroll', spy))
</script>

<template>
  <main class="page doc rules">
    <header class="pagehead back">
      <BackButton />
      <h1>规则</h1>
    </header>

    <div class="tocp phone-only" :class="{ open }">
      <button type="button" class="tocbtn" :aria-expanded="open" aria-controls="toc-phone" @click="open = !open">
        <span>目录</span><span class="cur">{{ title(current) }}</span>
      </button>
      <nav v-show="open" id="toc-phone" class="toc" aria-label="目录">
        <template v-for="part in PARTS" :key="part.label">
          <h4>{{ part.label }}</h4>
          <a v-for="[id, t] in part.items" :key="id" :href="`#${id}`" :class="{ on: current === id }" @click.prevent="go(id)">{{ t }}</a>
        </template>
      </nav>
    </div>

    <div class="body">
      <nav class="toc tocd desk-only" aria-label="目录">
        <template v-for="part in PARTS" :key="part.label">
          <h4>{{ part.label }}</h4>
          <a v-for="[id, t] in part.items" :key="id" :href="`#${id}`" :class="{ on: current === id }" :aria-current="current === id ? 'location' : undefined" @click.prevent="go(id)">{{ t }}</a>
        </template>
      </nav>

      <div class="content">
        <div class="part">基础</div>
        <section id="company" class="sec">
          <h2>公司与营收</h2>
          <ul>
            <li>每部作品是一家上市公司，总股本 10,000 股。25% 公开发行，75% 由制作委员会（真实世界的版权方）持有。</li>
            <li>每天的外部需求按三个市场的热度份额分给各公司：中国 50%（Bangumi 每集讨论）、欧美 30%（AniList 活跃度和新增收藏）、日本 20%（Annict 每集看过人数）。全市场越热，总需求越大。</li>
            <li>营收的一半是运营成本，付给外部。其余部分交 25% 企业所得税。</li>
            <li>税后利润的 90% 在每周日作为分红发给股东，其余留在公司账上，计入每股净资产。</li>
            <li>制作委员会分到的分红不再回到市场。</li>
          </ul>
        </section>

        <section id="hours" class="sec">
          <h2>交易时间</h2>
          <table class="t">
            <thead><tr><th>时段</th><th>时间</th><th>说明</th></tr></thead>
            <tbody>
              <tr><td>集合竞价</td><td class="n">{{ st?.auction_start ?? '08:45' }}–{{ st?.open ?? '09:00' }}</td><td>只收限价委托，{{ st?.open ?? '09:00' }} 统一成交</td></tr>
              <tr><td>连续交易</td><td class="n">{{ sessionText(st?.open, st?.close) }}</td><td>委托随到随成交</td></tr>
              <tr><td>日结</td><td class="n">{{ st?.daily_close ?? '06:30' }}</td><td>撤销未成交的委托，发放分红和利息</td></tr>
            </tbody>
          </table>
          <p>凌晨 {{ st?.day_rollover ?? '04:00' }} 以前的委托和成交计入前一个交易日，K 线、涨跌幅和分红登记也按交易日计算。</p>
        </section>

        <section id="orders" class="sec">
          <h2>委托与成交</h2>
          <ul>
            <li>价格优先、时间优先，按先进入盘口一方的价格成交。</li>
            <li>网页委托都是限价委托，价格默认为对手价：买入为卖 1，卖出为买 1。价格达到对手方报价的部分立即成交，其余部分进入盘口排队，可以撤单。</li>
            <li>市价委托只能通过 API 提交：从盘口最优价开始成交，未成交的部分立即撤销。</li>
            <li>没有成交的委托在当天日结时自动撤销。</li>
            <li>最少 1 股。当天买入的股票当天可以卖出。</li>
            <li>普通账户不能卖空，也不能借钱买入；开通信用账户后可以融资融券，见<a href="#margin" @click.prevent="go('margin')">融资融券</a>。</li>
          </ul>
          <h3>集合竞价</h3>
          <ul>
            <li>竞价时段只收限价委托，先不成交。页面显示当前的参考开盘价和可成交股数。</li>
            <li>{{ st?.open ?? '09:00' }} 按能成交股数最多的价格一次性撮合，这个价格就是当天的开盘价。没有可成交的委托时，以昨收为开盘价。</li>
            <li>收盘后的消息无论好坏，第二天所有人在同一时间按同一价格反应。</li>
          </ul>
        </section>

        <section id="fees" class="sec">
          <h2>费用与税</h2>
          <table class="t">
            <thead><tr><th>项目</th><th>费率</th><th>说明</th></tr></thead>
            <tbody>
              <tr><td>佣金</td><td class="n">{{ p(r?.fees.commission, 3) }}</td><td>买卖双方各付，没有每笔最低佣金</td></tr>
              <tr><td>印花税</td><td class="n">{{ p(r?.fees.stamp_duty) }}</td><td>只有卖方付</td></tr>
            </tbody>
          </table>
          <h3>股息红利税</h3>
          <table class="t">
            <thead><tr><th>持股时间</th><th>税率</th></tr></thead>
            <tbody><tr v-for="x in taxes" :key="x.k"><td>{{ x.k }}</td><td class="n">{{ x.v }}</td></tr></tbody>
          </table>
          <p>持股时间按先买先算。</p>
        </section>

        <section id="vcm" class="sec">
          <h2>冷静期</h2>
          <ul>
            <li>没有涨跌停，作品的好坏当天就能反映到股价上。价格短时间内波动过大时进入冷静期。</li>
            <li>当天第一次出现成交价比 {{ r?.vcm.window_minutes ?? 5 }} 分钟前的价格偏离超过 {{ p(r?.vcm.threshold, 0) }} 时，这笔不成交，股票进入 {{ r?.vcm.cool_minutes ?? 5 }} 分钟冷静期。期间只在参考价 ±{{ p(r?.vcm.threshold, 0) }} 以内成交，区间外的委托排队等待。</li>
            <li>之后以当天开盘价为基准，偏离超过 {{ vcmBands }} 时依次再次进入冷静期。</li>
            <li>限价委托的价格超过昨收 {{ r?.price_limit_multiple ?? 20 }} 倍时不予接受。</li>
          </ul>
        </section>

        <div class="part">新股与公司行为</div>
        <section id="ipo" class="sec">
          <h2>新股认购</h2>
          <ul>
            <li>新番在首播前约 3 周上市。发行价是按开播前热度估计的价值打九折。</li>
            <li>上市前 {{ r?.ipo.subscription_days ?? 3 }} 天开放认购，按发行价冻结资金。截止前可以修改股数或撤回。认购需要完成身份验证。</li>
            <li>认购截止当天日结时分配，次日上市交易。认购超过公开发售量时按比例分配，零头抽签，未分到的资金退回。没有售出的股份由制作委员会持有。</li>
          </ul>
          <h3>公开发售与回拨</h3>
          <table class="t">
            <thead><tr><th>超额认购倍数</th><th>公开发售占流通股</th></tr></thead>
            <tbody><tr v-for="x in clawback" :key="x.k"><td>{{ x.k }}</td><td class="n">{{ x.v }}</td></tr></tbody>
          </table>
        </section>

        <section id="schedule" class="sec">
          <h2>排期变化</h2>
          <ul>
            <li>未播出的各集按公开排期计入营收和估值：Bangumi 的各集播出日期，下一集以 AniList 的播出时间核对。已公布的休播从公布当天起计入。</li>
            <li>排期推后时发布公告，例如“第 5 集延期至 11 月 20 日”。之后各集顺延，营收随之推后，作品本身的热度不变。</li>
            <li>AniList 显示制作暂停时发布“暂停播出”公告。剩余各集的时间未定，估值按约 10 个月后恢复计算。</li>
            <li>续作公布时发布公告，估值中的续作从“可能有”变为“已公布”。</li>
          </ul>
        </section>

        <section id="corporate" class="sec">
          <h2>拆股、配股与回购</h2>
          <p>每家公司的管理层按以下公开规则行事。</p>
          <h3>拆股</h3>
          <ul>
            <li>收盘价连续 5 个交易日高于 100 星辉时公告，7 天后生效。每 1 股拆为 2、3、4、5 或 10 股，取拆后最接近全市场股价中位数的比例。</li>
            <li>持仓股数、持仓成本和欠券同比例换算，市值不变。</li>
          </ul>
          <h3>配股</h3>
          <ul>
            <li>续作公布时，公司先用账上现金支付制作费（续作这一季价值的 20%），不足部分按持仓向股东配股。</li>
            <li>配股价为公告当天收盘价的八折，每 10 股最多配 3 股。公告后 3 天内可以认购，第 3 天收盘时登记、缴款、发股，次日按除权参考价开始交易。</li>
            <li>配股权不能交易。没有认购的部分由制作委员会按除权参考价接手，差价以现金补给股东。融券卖出的一方按欠券向出借人支付同样的差价。</li>
          </ul>
          <h3>回购</h3>
          <ul>
            <li>每周日，账上闲置现金不少于市值 1% 且股价低于公司估计的每股价值时，公司公告回购：用闲置现金在 3 个月内买回股票并注销。</li>
            <li>回购价格不高于过去 30 个交易日均价的 1.5 倍，也不高于公司估计的价值。每天最多买入过去 20 个交易日日均成交量的 25%，开盘后和收盘前 30 分钟内不买。</li>
          </ul>
          <h3>K 线复权</h3>
          <ul>
            <li>K 线默认前复权：拆股、配股和每次分红之前的价格按比例换算，与现在的价格衔接。季番每周分红，不复权的 K 线会随分红持续走低。可以切换为不复权。</li>
          </ul>
        </section>

        <section id="delist" class="sec">
          <h2>退市</h2>
          <ul>
            <li>作品完结后营收迅速下降，完结后第一周约为平时的四成。</li>
            <li>已完结、营收很低并持续 8 周的公司退市，账上现金按股数返还给股东。</li>
          </ul>
        </section>

        <div class="part">进阶</div>
        <section id="margin" class="sec">
          <h2>融资融券</h2>
          <ul>
            <li>融资买入是借钱买入股票，融券卖出是借股票卖出。开通后原账户即为信用账户，现金和全部持仓都是担保物。</li>
            <li>开通条件：完成身份验证，净资产不低于初始资金的 50%，风险测评 C4 及以上，通过融资融券知识测试，签署《融资融券风险揭示书》。</li>
            <li>上市满 5 个交易日、昨收不低于 1 星辉、没有退市倒计时的股票可以融资买入或融券卖出。</li>
            <li>维持担保比例 =（现金 + 持仓市值）÷ 负债。</li>
          </ul>
          <table class="t">
            <thead><tr><th>项目</th><th>数值</th></tr></thead>
            <tbody>
              <tr><td>融资利率</td><td>政策利率 + 3%</td></tr>
              <tr><td>融券费率</td><td>政策利率 + 5%</td></tr>
              <tr><td>标的股票折算率</td><td>70%</td></tr>
              <tr><td>追加担保物</td><td>日结时低于 130%</td></tr>
              <tr><td>强制平仓</td><td>次日日结仍低于 130%；盘中低于 110%</td></tr>
              <tr><td>平仓目标</td><td>回到 150% 以上</td></tr>
            </tbody>
          </table>
          <ul>
            <li>追保和强制平仓期间不能新增融资融券，只能还款还券。</li>
            <li>持仓卖完后净资产仍为负时穿仓：信用账户关闭，欠款由破产重整承担，此后排行榜的累计收益率记为 −100%。</li>
          </ul>
        </section>

        <section id="futures" class="sec">
          <h2>热度期货</h2>
          <ul>
            <li>标的是一部作品 4 周（28 天）的平均热度，即合约期内每天公布的安易热度指数的平均值。计算方法见<RouterLink to="/rules/heat-index">指数编制方法</RouterLink>。</li>
            <li>每部在播作品同一时间交易一个合约。第 1 期从开播满一周开始挂牌，之后每期在上一期的最后一周挂牌。</li>
            <li>价格是市场对合约期平均热度的预期。押更热（买入）在平均热度高于成交价时盈利，押更冷（卖出）在低于成交价时盈利。每手每 1 点盈亏 10 星辉。</li>
            <li>每天日结时按结算价计算盈亏并划转。结算价取收盘前 30 分钟的成交均价；这段时间没有成交时，取做市商报价的中间价。</li>
            <li>合约期最后一天的指数公布后，按 28 天的平均值到期结算。到期前可以平仓。</li>
            <li>使用信用账户的保证金。开仓冻结初始保证金，约为合约价值的 15%。亏损计入维持担保比例，强制平仓时先平期货。</li>
            <li>单个合约的合约价值不超过净资产的 50%，全部初始保证金不超过净资产的 80%。</li>
            <li>交易条件：开通信用账户，风险测评 C4 及以上，通过热度期货知识测试，签署《热度期货风险揭示书》。只平仓的委托不受限制。</li>
          </ul>
        </section>

        <section id="funds" class="sec">
          <h2>指数基金</h2>
          <ul>
            <li>每只指数基金跟踪一个安易指数，按指数的成分和权重持有股票。</li>
            <li>申购按金额，至少 100 星辉；赎回按份额。提交后的下一个交易时段执行，收盘后按当天的单位净值确认。开始执行前可以撤单。</li>
            <li>单位净值从 1 星辉起，每天收盘后公布。盘中的今日估算按跟踪指数的实时点位推算。</li>
            <li>管理费每年 0.5%，每天从净值中扣除。不收申购费。份额持有不满 7 天的，赎回费 1.5%，归入基金。</li>
            <li>基金买卖股票的佣金、印花税和对成交价的影响，由当天申购、赎回的人承担。基金收到的分红不缴股息税，计入净值。</li>
            <li>基金份额按最新净值计入总资产和排行。持有份额时不能申请破产重整或注销账号；有融资融券负债时不能申购。</li>
          </ul>
        </section>

        <div class="part">账户与市场环境</div>
        <section id="account" class="sec">
          <h2>账户与利息</h2>
          <ul>
            <li>注册获得 {{ money(r?.initial_cash ?? 10000000, 0) }} 星辉。</li>
            <li>现金每天按存款利率计息，存款利率 = 政策利率 − 1%。</li>
            <li>每周日，财政把本周税收的 25% 平均发给符合条件的玩家（财政补贴）：完成身份验证、注册满 7 天、过去 28 天内有成交。</li>
            <li>破产重整：总资产低于初始资金的 {{ p(r?.restructure.threshold_ratio ?? 0.1, 0) }} 时可以申请，每季 {{ r?.restructure.per_season ?? 1 }} 次。持仓按最新价卖给系统，现金补足到初始资金，排行榜的累计收益率连续计算。</li>
          </ul>
        </section>

        <section id="central-bank" class="sec">
          <h2>央行</h2>
          <ul>
            <li>每周日，央行查看过去 28 天货币总量（M2）的年化增速：高于 20% 加息，低于 20% 降息，每次最多 0.25%。</li>
            <li>利率影响存款利息和股票估值。利率越高，未来分红的价值越低，长篇连载受影响最大。</li>
          </ul>
        </section>

        <section id="indices" class="sec">
          <h2>指数</h2>
          <ul>
            <li>市场页的指数按全收益计算：分红当天把每股分红加回，拆股、配股不改变指数。</li>
            <li>成分为当天在市的股票，退市的股票退市前的涨跌照样计入。</li>
            <li>按流通市值加权，制作委员会和公司自己持有的股份不计入。除番剧综合指数外，单只股票的权重不超过 25%，超出部分按市值比例分给其他成分股。</li>
            <li>热度期货使用的安易热度指数，计算方法见<RouterLink to="/rules/heat-index">指数编制方法</RouterLink>。</li>
          </ul>
        </section>

        <section id="npc" class="sec">
          <h2>市场里的 NPC</h2>
          <ul>
            <li><b>做市商</b>：每支股票一个，在自己估计的价格两边挂单。资金和持股有限，卖出越多报价越高，也会看错和亏损。</li>
            <li><b>制作委员会</b>：股价远高于它认为的价值时少量减持。</li>
            <li><b>散户</b>：小额买卖，偏好正在被讨论的作品。首播当晚、发出热度快讯、进入涨跌幅榜的股票更容易被买入，这种热情几小时内消退。</li>
            <li><b>市场情绪</b>：全市场的情绪以几周为周期冷热交替，市场页显示为冷淡、平稳或火热。价格偏离价值后，看重价值的 NPC 会把它拉回。</li>
            <li>系统内部的估值不公开。公开的是营收、分红、集数和观众数等原始数据。</li>
          </ul>
        </section>

        <p class="foot">星辉是安易内的虚拟资产，与真实货币无关，不能充值或提现。作品数据来自 <a href="https://anilist.co" target="_blank" rel="noopener">AniList</a>、<a href="https://bgm.tv" target="_blank" rel="noopener">Bangumi</a>（条目信息 CC BY-SA）、<a href="https://annict.com" target="_blank" rel="noopener">Annict</a> 的公开统计。</p>
      </div>
    </div>
  </main>
</template>

<style scoped>
.rules { max-width: 1060px; }
.sec { scroll-margin-top: 56px; }
.sec h3 { font-size: 14px; font-weight: 700; margin: 14px 0 6px; }
.sec > p { font-size: 14px; line-height: 1.75; color: var(--ink-2); }
.sec > * + p, .sec > table + ul, .sec > ul + table { margin-top: 8px; }
.sec b { color: var(--ink); }
.sec a { color: var(--accent); }
.part { padding: 22px var(--gutter) 6px; font-size: 12px; color: var(--muted); letter-spacing: 0.04em; }
.part + .sec { border-top-width: 2px; }
.t { width: 100%; border-collapse: collapse; font-size: 13px; }
.t th { text-align: left; font-weight: 400; font-size: 12px; color: var(--muted); padding: 6px 12px 6px 0; border-bottom: 1px solid var(--hair); white-space: nowrap; }
.t td { padding: 9px 12px 9px 0; border-bottom: 1px solid var(--hair-2); vertical-align: top; line-height: 1.6; color: var(--ink-2); }
.t td:first-child { white-space: nowrap; color: var(--ink); }
.t td.n { font-family: var(--font-num); font-variant-numeric: tabular-nums; white-space: nowrap; color: var(--ink); }

/* contents */
.toc h4 { margin: 10px 0 4px; font-size: 12px; font-weight: 400; color: var(--muted); }
.toc h4:first-child { margin-top: 0; }
.toc a { display: block; padding: 5px 0 5px 10px; font-size: 14px; color: var(--ink-2); border-left: 2px solid transparent; }
.toc a.on { color: var(--ink); font-weight: 700; border-left-color: var(--ink); }
.tocp { position: sticky; top: 0; z-index: 5; background: var(--ground); border-bottom: 1px solid var(--hair); }
.tocbtn { width: 100%; display: flex; justify-content: space-between; align-items: center; padding: 12px var(--gutter); border: 0; background: none; font-size: 14px; color: var(--ink); }
.tocbtn .cur { font-size: 13px; color: var(--muted); }
#toc-phone { max-height: 60vh; overflow-y: auto; padding: 0 var(--gutter) 12px; }
@media (min-width: 1024px) {
  .body { display: grid; grid-template-columns: 200px minmax(0, 1fr); column-gap: 40px; padding: 0 32px; }
  .tocd { position: sticky; top: calc(var(--desk-header-h) + 20px); align-self: start; max-height: calc(100vh - var(--desk-header-h) - 40px); overflow-y: auto; padding-top: 22px; }
  .content .sec, .content .part, .content .foot { padding-left: 0; padding-right: 0; }
  .sec { scroll-margin-top: calc(var(--desk-header-h) + 12px); }
}
</style>
