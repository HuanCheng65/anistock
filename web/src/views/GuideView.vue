<script setup lang="ts">
// 新手指南: how to play, in the order a new player does things. Rule details are not repeated here: each section
// links to its section of 规则.
import { RouterLink, useRouter } from 'vue-router'
import BackButton from '../components/BackButton.vue'
import Icon from '../components/Icon.vue'
import SiteFooter from '../components/SiteFooter.vue'
import { KYC_UNLOCKS } from '../kyc'
import { useGuide } from '../stores/guide'
import { useMarket } from '../stores/market'
import { useSession } from '../stores/session'

const session = useSession()
const router = useRouter()
const market = useMarket()
const guide = useGuide()
// the page guides once more: on the stock with the most turnover today
function replay() {
  guide.again()
  const top = [...(market.stocks ?? [])].sort((a, b) => b.turnover - a.turnover)[0]
  router.push(top ? `/stocks/${top.stock.id}` : '/stocks')
}
</script>

<template>
  <main class="page doc guide">
    <header class="pagehead back">
      <BackButton />
      <h1>新手指南</h1>
    </header>

    <div class="intro">
      <p>安易是动画作品的模拟交易所。每部在播或即将开播的作品都有一支股票，用星辉买卖，价格由玩家之间的买卖决定。</p>
      <p>看好一部作品就买入，不看好就卖出。作品的播出和讨论热度会影响其他玩家的判断，股价随之涨跌。收益来自股价涨跌和分红，排行榜按收益率排名。</p>
      <p>熟悉以后，还可以认购新股、交易热度期货、申购指数基金，见文末“更多玩法”。</p>
    </div>

    <section id="first-trade" class="sec">
      <h2>第一笔交易</h2>
      <ol class="steps">
        <li>在“行情”中找到一部作品，打开它的股票页。</li>
        <li>看盘口：<b>卖 1</b> 是目前最低的卖价，<b>买 1</b> 是目前最高的买价。</li>
        <li>选择“买入”，填写股数。价格默认为卖 1，按这个价格提交会立即成交。</li>
        <li>价格低于卖 1 的委托进入盘口排队，等有人按这个价格卖出才成交。当天日结时没有成交的部分自动撤销。</li>
      </ol>
      <RouterLink to="/rules#orders" class="more">委托与成交 ›</RouterLink>
    </section>

    <section class="sec">
      <h2>买入以后</h2>
      <ul>
        <li>股价是最近一笔成交的价格，随玩家的买卖变化。没有涨跌停。</li>
        <li>每周日，公司把利润的大部分作为分红发放，按持股数计入可用资金。</li>
        <li>持有满 4 周的分红免税。不满 4 周交 10%，当天买入的交 20%。</li>
        <li>季番的营收集中在播出期间。剩余集数越少，以后的分红越少，股价通常随之走低。看盈亏时，把已经收到的分红一起算进去。</li>
      </ul>
      <RouterLink to="/rules#fees" class="more">费用与税 ›</RouterLink>
    </section>

    <section class="sec">
      <h2>怎么判断</h2>
      <ul>
        <li><b>热度</b>：股票页的“热度”按中国、欧美、日本三个市场显示每天的观看和讨论。</li>
        <li><b>营收</b>：“经营”显示每天的营收和每周的分红。</li>
        <li><b>剩余集数</b>：股票页顶部显示已播和总集数。</li>
        <li><b>观众规模</b>：开播前，Bangumi 的“想看”人数和 AniList 的收藏数是判断热度的主要依据。</li>
      </ul>
      <p class="note">系统不公布合理价格。做市商有自己的估计，也会看错。</p>
    </section>

    <section class="sec">
      <h2>新股认购</h2>
      <p class="note">新番在首播前约 3 周上市，上市前 3 天可以按发行价认购。认购超过发行量时按比例分配，零头抽签。认购需要完成身份验证。</p>
      <RouterLink to="/rules#ipo" class="more">新股认购规则 ›</RouterLink>
    </section>

    <section class="sec">
      <h2>身份验证</h2>
      <p class="note">在 QQ 里私聊机器人绑定账号即完成身份验证，一个 QQ 只能绑定一个账号。买卖股票不需要身份验证，以下功能需要：</p>
      <dl class="unlocks">
        <div v-for="[k, v] in KYC_UNLOCKS" :key="k"><dt>{{ k }}</dt><dd>{{ v }}</dd></div>
      </dl>
      <RouterLink v-if="session.me && !session.me.verification.verified" to="/settings" class="btn small">去验证</RouterLink>
    </section>

    <section class="sec">
      <h2>更多玩法</h2>
      <nav class="adv" aria-label="更多玩法">
        <RouterLink to="/rules#margin"><b>融资融券</b><span>借钱买入，或借股票卖出。放大盈亏，有强制平仓的风险。</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
        <RouterLink to="/rules#futures"><b>热度期货</b><span>判断一部作品接下来 4 周的平均热度，押更热或更冷。</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
        <RouterLink to="/rules#funds"><b>指数基金</b><span>按指数持有一篮子股票，申购和赎回按当天的单位净值。</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
      </nav>
    </section>

    <div class="cta">
      <RouterLink to="/stocks" class="btn">去看行情</RouterLink>
      <RouterLink v-if="!session.me" :to="{ name: 'login', query: { register: '1' } }" class="btn ghost">注册</RouterLink>
      <RouterLink to="/rules" class="btn ghost">规则</RouterLink>
      <button type="button" class="btn ghost" @click="replay">重看引导</button>
    </div>
    <SiteFooter />
  </main>
</template>

<style scoped>
.intro { padding: 16px var(--gutter) 18px; display: flex; flex-direction: column; gap: 8px; }
.intro p { font-size: 15px; line-height: 1.75; color: var(--ink-2); text-wrap: pretty; }
.intro p:first-child { color: var(--ink); }
.sec { scroll-margin-top: 64px; }
.sec .steps { counter-reset: s; list-style: none; padding-left: 0; gap: 0; }
.steps li { counter-increment: s; position: relative; padding: 10px 0 10px 34px; border-top: 1px solid var(--hair-2); }
.steps li:first-child { border-top: 0; }
.steps li::before {
  content: counter(s); position: absolute; left: 0; top: 13px; width: 22px; height: 22px; display: grid; place-items: center;
  background: var(--ink); color: var(--ground); font-weight: 600; font-size: 12px; font-family: var(--font-num);
}
.sec b { color: var(--ink); }
.note { font-size: 14px; line-height: 1.75; color: var(--ink-2); }
ul + .note { margin-top: 8px; }
.more { display: inline-block; margin-top: 8px; font-size: 13px; color: var(--accent); }
.unlocks { margin: 8px 0 12px; border-top: 1px solid var(--hair-2); }
.unlocks div { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding: 8px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; }
.unlocks dd { margin: 0; font-size: 12px; color: var(--muted); text-align: right; }
.adv { border-top: 1px solid var(--hair-2); }
.adv a { display: grid; grid-template-columns: 5.5em minmax(0, 1fr) 16px; gap: 10px; align-items: baseline; padding: 10px 0; border-bottom: 1px solid var(--hair-2); }
.adv b { font-size: 14px; }
.adv span { font-size: 13px; line-height: 1.6; color: var(--ink-2); }
.chev { color: var(--muted); align-self: center; }
.cta { display: flex; flex-wrap: wrap; gap: 8px; padding: 18px var(--gutter); border-top: 1px solid var(--rule); }
</style>
