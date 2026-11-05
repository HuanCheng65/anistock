<script setup lang="ts">
import { RouterLink } from 'vue-router'
import { useSession } from '../stores/session'
const session = useSession()
</script>

<template>
  <main class="page doc">
    <header>
      <span class="label">新手指南 · 五分钟</span>
      <h1>每部番都是一家公司，你买的是它的股份</h1>
    </header>

    <ol class="steps">
      <li>
        <h2>番剧的热度就是公司的营收</h2>
        <p>系统每天读取真实世界三个平台的数据：欧美的 AniList、中国的 Bangumi、日本的 Annict。一部番在这三个地方越火（越多人在追、在讨论、在标记看过），它当天分到的营收就越多。</p>
      </li>
      <li>
        <h2>营收变成分红，每周日发给股东</h2>
        <p>公司扣掉成本、交完税，把利润的九成在每周日按股数分给股东，直接打进你的现金。持股超过一季免股息税，持有不满一周要交 20%。</p>
      </li>
      <li>
        <h2>股价由大家买卖决定</h2>
        <p>和真实股市一样，价格是买卖双方挂单撮合出来的。觉得这部番会火、而现在价格还便宜，就买；觉得被炒过头了，就卖。没有涨跌停，但价格几分钟内波动太大时会进入 5 分钟的“冷静期”。</p>
      </li>
      <li>
        <h2>每天早上先集合竞价</h2>
        <p>08:45–09:00 可以先挂限价单，09:00 所有人按同一个开盘价成交。深夜播出的神回，第二天大家在同一起跑线上反应。</p>
      </li>
      <li>
        <h2>季番的股价会随着播出慢慢“分掉”</h2>
        <p>一季番的赚钱期只有几个月，完结后热度掉得很快。公司的价值是一周一周作为分红发给你的，所以股价通常会随着集数推进走低——这不一定是亏，要看“我的”页面里的<b>含分红总盈亏</b>。</p>
      </li>
      <li>
        <h2>新番开播前认购新股</h2>
        <p>新番在首播前约 3 周上市，上市前 3 天可以按发行价认购。开播前没人知道它会不会火，这正是靠眼光赚钱的时候。认购的人多了要按比例分配加抽签，去“新股认购”页看看。</p>
      </li>
    </ol>

    <section class="panel tips">
      <h2>第一笔交易怎么下</h2>
      <ol>
        <li>在行情页点开一部你在追的番。</li>
        <li>在交易面板选“买入”，输入股数。不知道填什么价格，就点盘口里“卖1”的价格，它会自动填入。</li>
        <li>点“买入”。能立刻成交就成交；成交不了的部分会挂在盘口里等，随时可以撤单。</li>
        <li>没成交的委托在每天早上的日结时自动撤销。没有涨跌停，大单用限价单更安全。</li>
      </ol>
    </section>

    <section class="panel tips">
      <h2>想玩得更深</h2>
      <ul>
        <li>看个股页的<b>每周营收</b>：三个市场分开画，哪部番在中国火、在欧美冷，一目了然。</li>
        <li>看<b>观众规模</b>：Bangumi 的“想看”人数、AniList 的收藏数，是开播前判断热度的主要线索。</li>
        <li>算一算：一部番还剩几集、每周分多少，股价是不是太贵了？系统不会告诉你“合理价格”，做市商心里有一个，但它也会看错。</li>
        <li>留意<b>央行利率</b>：利率越高，现金放着不动的利息越多，股票的吸引力越低。长篇番对利率尤其敏感。</li>
      </ul>
    </section>

    <div class="cta">
      <RouterLink to="/ipo" class="btn">新股认购</RouterLink>
      <RouterLink v-if="!session.me" to="/login" class="btn primary">注册领取 10 万币</RouterLink>
      <RouterLink to="/" class="btn">去看行情</RouterLink>
      <RouterLink to="/rules" class="btn">完整规则</RouterLink>
    </div>
  </main>
</template>

<style scoped>
.doc { max-width: 720px; }
header { display: flex; flex-direction: column; gap: 6px; padding: 8px 2px; }
header h1 { font-size: 22px; font-weight: 700; }
.steps { list-style: none; margin: 0; padding: 0; counter-reset: s; display: flex; flex-direction: column; gap: 10px; }
.steps li { counter-increment: s; position: relative; padding: 12px 12px 12px 46px; background: var(--surface); border: 1px solid var(--rule); border-radius: var(--radius); }
.steps li::before {
  content: counter(s); position: absolute; left: 12px; top: 12px; width: 24px; height: 24px; display: grid; place-items: center;
  background: var(--seal); color: #fff; border-radius: 3px; font-weight: 700; font-size: 13px; font-family: var(--font-num);
}
.steps h2 { font-size: 15px; font-weight: 600; margin-bottom: 4px; }
.steps p { color: var(--ink-2); font-size: 14px; }
.tips { padding: 12px 14px; }
.tips h2 { font-size: 15px; font-weight: 600; margin-bottom: 6px; }
.tips ol, .tips ul { margin: 0; padding-left: 1.3em; color: var(--ink-2); font-size: 14px; display: flex; flex-direction: column; gap: 4px; }
.cta { display: flex; flex-wrap: wrap; gap: 8px; }
</style>
