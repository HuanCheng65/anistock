<script setup lang="ts">
// 作品: each work's broadcast schedule (aired, upcoming, 延期 marked quietly), the per-episode discussion of the
// latest work, the sequel and the audience numbers. Dates only from public sources; an undated episode says 待定.
import { computed } from 'vue'
import type { StockDetail, Work, WorksResponse } from '../../api'
import { big, shortDay } from '../../format'
import CommentBars from './CommentBars.vue'

const props = defineProps<{ s: StockDetail; works: WorksResponse | null; failed: boolean; today?: string }>()
defineEmits<{ retry: [] }>()

// newest first; the current work (on air, else the next season, else the latest) shows its schedule, the other
// finished works of the series fold to one line each (a 366-episode first series would fill the page)
const list = computed(() => [...(props.works?.works ?? [])].sort((a, b) => (b.first_air ?? '').localeCompare(a.first_air ?? '')))
const many = computed(() => list.value.length > 1)
const current = computed(() => {
  const l = list.value
  return (l.find((w) => w.status === 'airing' || w.status === 'halted') ?? [...l].reverse().find((w) => w.status === 'upcoming') ?? l[0])?.id
})
const folded = (w: Work) => many.value && w.id !== current.value && w.status === 'finished'
function span(w: Work): string {
  const from = w.first_air?.slice(0, 4)
  const to = [...w.schedule].reverse().find((e) => e.aired && e.date)?.date?.slice(0, 4)
  return from && to && to !== from ? `${from}–${to}` : from ?? ''
}
/** the work whose discussion is shown: the latest with any */
const talk = computed(() => [...list.value].reverse().find((w) => w.discussion.length > 1) ?? null)

function md(day: string): string {
  const [y, m, d] = day.split('-').map(Number)
  return d ? `${y} 年 ${m} 月 ${d} 日` : m ? `${y} 年 ${m} 月` : `${y} 年`
}
function status(w: Work): string {
  if (w.status === 'halted') return '暂停播出'
  if (w.status === 'upcoming') {
    if (!w.first_air) return '未开播'
    const [y, m, d] = w.first_air.split('-').map(Number)
    const year = props.today && String(y) !== props.today.slice(0, 4) ? `${y} 年 ` : ''
    return `${year}${m} 月 ${d} 日开播`
  }
  if (w.status === 'finished') return `已完结 · 共 ${w.episodes_aired} 集`
  if (w.is_long_runner) return `第 ${w.episodes_aired} 集`
  return w.episodes ? `${w.episodes_aired} / ${w.episodes}` : `第 ${w.episodes_aired} 集`
}
/** a cell's date: 今天 or MM-dd (the year is in the label and, for a next-year premiere, the status line) */
function cellDay(day: string): string {
  return day === props.today ? '今天' : day.slice(5)
}
/** the first episode still to air */
function nextEp(w: Work): number | null {
  return w.schedule.find((e) => !e.aired)?.episode ?? null
}
function epLabel(e: Work['schedule'][number]): string {
  const when = e.date ? shortDay(e.date, props.today) : '日期待定'
  const state = e.aired ? '已播出' : '未播出'
  return `第 ${e.episode} 集，${when}，${state}${e.delayed ? '，延期' : ''}`
}

const sequel = computed(() => {
  const q = props.works?.sequel
  if (!q) return null
  switch (q.status) {
    case 'none':
      return { v: '未公布', t: null }
    case 'announced':
      return { v: q.premiere ? `已公布 · ${md(q.premiere)}` : '已公布', t: q.title }
    case 'dated':
      return { v: `定档 ${q.premiere ? md(q.premiere) : ''}`.trim(), t: q.title }
    default:
      return { v: '暂无消息', t: null }
  }
})

// public audience numbers (collector snapshots); a source without data is left out
const audience = computed(() => {
  const a = props.s.audiences
  const out: { k: string; v: string }[] = []
  if (a.bangumi?.score != null) out.push({ k: 'Bangumi 评分', v: a.bangumi.score.toFixed(1) })
  if (a.bangumi?.collection) out.push({ k: 'Bangumi 在看', v: big(a.bangumi.collection.doing) })
  if (a.anilist?.score != null) out.push({ k: 'AniList 评分', v: String(a.anilist.score) })
  if (a.anilist?.popularity != null) out.push({ k: 'AniList 人气', v: big(a.anilist.popularity) })
  if (a.annict?.watchers != null) out.push({ k: 'Annict 观看', v: big(a.annict.watchers) })
  return out
})
</script>

<template>
  <div class="works">
    <div class="sched">
      <template v-if="works">
        <p v-if="many" class="count">系列共 {{ list.length }} 部</p>
        <template v-for="w in list" :key="w.id">
        <details v-if="folded(w)" class="work fold">
          <summary class="wh">
            <h3 class="h3 clamp1">{{ w.title }}</h3>
            <span class="st">{{ span(w) }} · 已完结 {{ w.episodes_aired }} 集</span>
          </summary>
          <ol class="eps" :class="{ long: w.is_long_runner }">
            <li v-for="e in w.schedule" :key="e.episode" class="ep aired">
              <span class="sr-only">{{ epLabel(e) }}</span>
              <span class="n num" aria-hidden="true">{{ e.episode }}</span>
              <span class="d num" aria-hidden="true">{{ e.date ? cellDay(e.date) : '待定' }}</span>
            </li>
          </ol>
        </details>
        <section v-else class="work" :aria-label="many ? w.title : '播出排期'">
          <header v-if="many || w.status !== 'airing'" class="wh">
            <h3 v-if="many" class="h3 clamp1">{{ w.title }}</h3>
            <span class="st" :class="{ quiet: !many }">{{ status(w) }}</span>
          </header>
          <ol class="eps" :class="{ long: w.is_long_runner }">
            <li v-for="e in w.schedule" :key="e.episode" class="ep"
                :class="{ aired: e.aired, next: e.episode === nextEp(w), delayed: e.delayed, undated: !e.date }">
              <span class="sr-only">{{ epLabel(e) }}</span>
              <span class="n num" aria-hidden="true">{{ e.episode }}</span>
              <span class="d num" aria-hidden="true">{{ e.date ? cellDay(e.date) : '待定' }}</span>
              <span v-if="e.delayed" class="dl" aria-hidden="true">延期</span>
            </li>
          </ol>
          <p v-if="!w.schedule.length" class="none">暂无排期</p>
        </section>
        </template>
        <p v-if="!list.length" class="none">暂无排期</p>
      </template>
      <div v-else-if="failed" class="none err">
        无法加载排期
        <button type="button" class="linkbtn" @click="$emit('retry')">重试</button>
      </div>
      <div v-else class="skel" style="height: 120px"></div>
    </div>

    <div class="side">
      <section v-if="talk" class="blk" aria-labelledby="w-talk">
        <h3 id="w-talk" class="h3">单集讨论</h3>
        <CommentBars :items="talk.discussion" />
      </section>
      <dl v-if="sequel" class="row">
        <dt>续作</dt>
        <dd>
          <span>{{ sequel.v }}</span>
          <span v-if="sequel.t" class="sub clamp1">《{{ sequel.t }}》</span>
        </dd>
      </dl>
      <section v-if="audience.length" class="blk" aria-labelledby="w-aud">
        <h3 id="w-aud" class="h3">观众</h3>
        <dl class="facts">
          <div v-for="x in audience" :key="x.k"><dt>{{ x.k }}</dt><dd>{{ x.v }}</dd></div>
        </dl>
      </section>
    </div>
  </div>
</template>

<style scoped>
.works { display: flex; flex-direction: column; gap: 22px; }
.sched { display: flex; flex-direction: column; gap: 18px; min-width: 0; }
.work { display: flex; flex-direction: column; gap: 8px; }
.wh { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; }
.h3 { font-family: var(--font-serif); font-size: 15px; font-weight: 900; min-width: 0; }
.st { font-size: 13px; color: var(--ink-2); white-space: nowrap; }
.st.quiet { color: var(--muted); }
.count { font-size: 13px; color: var(--muted); }
.fold { gap: 0; }
.fold > summary { list-style: none; cursor: pointer; padding: 8px 0; border-top: 1px solid var(--hair); }
.fold > summary::-webkit-details-marker { display: none; }
.fold > summary .h3 { font-size: 14px; font-weight: 700; }
.fold[open] > summary { margin-bottom: 8px; }
.none { font-size: 13px; color: var(--muted); padding: 8px 0; }
.err { display: flex; gap: 10px; align-items: baseline; }

.eps { display: grid; grid-template-columns: repeat(auto-fill, minmax(52px, 1fr)); gap: 10px 6px; }
.ep { display: flex; flex-direction: column; gap: 1px; padding-top: 6px; border-top: 2px solid var(--hair); min-width: 0; }
.ep .n { font-size: 14px; color: var(--muted); line-height: 1.3; }
.ep .d { font-size: 11px; color: var(--muted); white-space: nowrap; }
.ep.aired { border-top-color: var(--accent); }
.ep.aired .n { color: var(--ink); }
.ep.next { border-top-style: dashed; border-top-color: var(--accent); }
.ep.next .n, .ep.next .d { color: var(--ink); }
.ep.undated .d { color: var(--muted); }
.ep.delayed .d { text-decoration: underline dotted var(--base-line); text-underline-offset: 3px; }
.dl { font-size: 11px; color: var(--ink-2); line-height: 1.2; }
.eps.long { grid-template-columns: repeat(auto-fill, minmax(64px, 1fr)); }

.side { display: flex; flex-direction: column; gap: 18px; min-width: 0; }
.blk { display: flex; flex-direction: column; gap: 8px; }
.row { display: flex; justify-content: space-between; align-items: baseline; gap: 16px; padding: 10px 0; border-top: 1px solid var(--hair); border-bottom: 1px solid var(--hair); font-size: 14px; }
.row dt { color: var(--muted); font-size: 13px; flex: none; }
.row dd { display: flex; flex-direction: column; align-items: flex-end; gap: 2px; min-width: 0; text-align: right; }
.row .sub { font-size: 12px; color: var(--ink-2); max-width: 100%; }
.side .facts { border-top: 1px solid var(--hair); }

@media (min-width: 1024px) {
  .works { display: grid; grid-template-columns: minmax(0, 1.5fr) minmax(0, 1fr); gap: 32px; align-items: start; }
  .eps { grid-template-columns: repeat(auto-fill, minmax(60px, 1fr)); }
  .side .facts { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
</style>
