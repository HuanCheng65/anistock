<script setup lang="ts">
import { RouterLink } from 'vue-router'
import BackButton from '../components/BackButton.vue'
import { useSession } from '../stores/session'
const session = useSession()
</script>

<template>
  <main class="page doc">
    <header class="pagehead back">
      <BackButton />
      <h1>新手指南</h1>
    </header>
    <p class="lede serif">每部番都是一家公司，你买的是它的股份</p>

    <ol class="steps">
      <li>
        <h2>番剧的热度就是公司的营收</h2>
        <p>系统每天读取真实世界三个平台的数据：欧美的 AniList、中国的 Bangumi、日本的 Annict。一部番在这三个地方越火（越多人在追、在讨论、在标记看过），它当天分到的营收就越多。</p>
      </li>
      <li>
        <h2>营收变成分红，每周日发给股东</h2>
        <p>公司扣掉成本、交完税，把利润的九成在每周日按股数分给股东，直接打进你的现金。持股满 4 周的分红免税，不满 4 周交 10%，当天买入的交 20%。</p>
      </li>
      <li>
        <h2>股价由大家买卖决定</h2>
        <p>和真实股市一样，价格是买卖双方挂单撮合出来的。觉得这部番会火、而现在价格还便宜，就买；觉得被炒过头了，就卖。没有涨跌停，但价格几分钟内波动太大时会进入 5 分钟的“冷静期”。</p>
      </li>
      <li>
        <h2>每天早上先集合竞价</h2>
        <p>08:45–09:00 可以先挂限价单，09:00 所有人按同一个开盘价成交。收盘后出来的消息，第二天所有人按同一价格反应。</p>
      </li>
      <li>
        <h2>季番的股价会随着播出慢慢“分掉”</h2>
        <p>一季番的赚钱期只有几个月，完结后热度掉得很快。公司的价值是一周一周作为分红发给你的，所以股价通常会随着集数推进走低——这不一定是亏，要把收到的<b>分红</b>一起算进盈亏。</p>
      </li>
      <li>
        <h2>新番开播前认购新股</h2>
        <p>新番在首播前约 3 周上市，上市前 3 天可以按发行价认购。开播前没人知道它会不会火，这正是靠眼光赚钱的时候。认购的人多了要按比例分配加抽签，详见“新股认购”。</p>
      </li>
    </ol>

    <section class="sec">
      <h2>第一笔交易怎么下</h2>
      <ol>
        <li>在“股票”里找到一部你在追的番，打开它的股票页。</li>
        <li>选“买入”，填写数量。价格可以直接选盘口里最低的卖价，它会填入价格栏。</li>
        <li>提交后能成交的部分立即成交，其余部分留在盘口等待，随时可以撤单。</li>
        <li>没成交的委托在每天早上的日结时自动撤销。没有涨跌停，大额委托用限价委托更稳妥。</li>
      </ol>
    </section>

    <section class="sec">
      <h2>想玩得更深</h2>
      <ul>
        <li>看股票页的<b>营收</b>：三个市场分开画，哪部番在中国火、在欧美冷，一目了然。</li>
        <li>看<b>观众规模</b>：Bangumi 的“想看”人数、AniList 的收藏数，是开播前判断热度的主要线索。</li>
        <li>算一算：一部番还剩几集、每周分多少，股价是不是太贵了？系统不会告诉你“合理价格”，做市商心里有一个，但它也会看错。</li>
        <li>留意<b>央行利率</b>：利率越高，现金放着不动的利息越多，股票的吸引力越低。长篇番对利率尤其敏感。</li>
      </ul>
    </section>

    <div class="cta">
      <RouterLink to="/stocks" class="btn">去看股票</RouterLink>
      <RouterLink v-if="!session.me" to="/login" class="btn ghost">注册</RouterLink>
      <RouterLink to="/ipo" class="btn ghost">新股认购</RouterLink>
      <RouterLink to="/rules" class="btn ghost">规则</RouterLink>
    </div>
  </main>
</template>

<style scoped>
.steps { counter-reset: s; }
.steps li { counter-increment: s; position: relative; padding: 14px var(--gutter) 14px calc(var(--gutter) + 34px); border-top: 1px solid var(--hair); }
.steps li::before {
  content: counter(s); position: absolute; left: var(--gutter); top: 15px; width: 22px; height: 22px; display: grid; place-items: center;
  background: var(--brand); color: #fff; font-weight: 600; font-size: 12px; font-family: var(--font-num);
}
.steps h2 { font-family: var(--font-serif); font-size: 17px; font-weight: 900; margin-bottom: 4px; }
.steps p { color: var(--ink-2); font-size: 14px; line-height: 1.7; }
.cta { display: flex; flex-wrap: wrap; gap: 8px; padding: 18px var(--gutter); }
</style>
