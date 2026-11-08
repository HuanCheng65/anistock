<script setup lang="ts">
// 规则, 开发者说明, 指数编制方法: the Markdown in web/content/, rendered at build time (docs.plugin.ts), which is also
// published as /rules.md etc. for agents. This page adds what is not part of the document: the contents (documents
// with parts) marking the section being read, in-app navigation for its links, and live notices.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import api from '../../content/api.md'
import heat from '../../content/heat-index.md'
import rules from '../../content/rules.md'
import BackButton from '../components/BackButton.vue'
import SiteFooter from '../components/SiteFooter.vue'
import { useSession } from '../stores/session'

const props = defineProps<{ doc: 'rules' | 'api' | 'heat' }>()
const DOCS = { rules, api, heat }
const d = computed(() => DOCS[props.doc])
const router = useRouter()
const session = useSession()
// API orders paused by the operator: live state, so on the page rather than in the document
const paused = computed(() => props.doc === 'api' && !!session.me && session.me.api_trading?.enabled === false)

// the contents, grouped by part
const parts = computed(() => {
  const out: { label: string; items: { id: string; title: string }[] }[] = []
  for (const s of d.value.sections) {
    if (s.part == null) continue
    if (out[out.length - 1]?.label !== s.part) out.push({ label: s.part, items: [] })
    out[out.length - 1].items.push(s)
  }
  return out
})
const hasToc = computed(() => parts.value.length > 0)
const title = (id: string) => d.value.sections.find((s) => s.id === id)?.title ?? ''

// the section being read: the last one whose heading has passed the top quarter of the window
const current = ref('')
const open = ref(false)
let frame = 0
function spy() {
  cancelAnimationFrame(frame)
  frame = requestAnimationFrame(() => {
    let cur = d.value.sections[0]?.id ?? ''
    for (const { id } of d.value.sections) {
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

// links in the document: anchors scroll, pages of the app open in the app, the rest (new tab, .md, /api/) as usual
function onClick(e: MouseEvent) {
  if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return
  const a = (e.target as HTMLElement).closest('a')
  const href = a?.getAttribute('href')
  if (!a || !href || a.target === '_blank') return
  if (href.startsWith('#')) {
    e.preventDefault()
    go(href.slice(1))
  } else if (href.startsWith('/') && !href.startsWith('/api/') && !/\.(md|txt|json)$/.test(href.split('#')[0])) {
    e.preventDefault()
    router.push(href)
  }
}

onMounted(() => {
  if (!hasToc.value) return
  window.addEventListener('scroll', spy, { passive: true })
  spy()
})
onBeforeUnmount(() => window.removeEventListener('scroll', spy))
</script>

<template>
  <main class="page doc docpage" :class="{ withtoc: hasToc }">
    <header class="pagehead back">
      <BackButton />
      <h1>{{ d.title }}</h1>
    </header>

    <div v-if="hasToc" class="tocp phone-only" :class="{ open }">
      <button type="button" class="tocbtn" :aria-expanded="open" aria-controls="toc-phone" @click="open = !open">
        <span>目录</span><span class="cur">{{ title(current) }}</span>
      </button>
      <nav v-show="open" id="toc-phone" class="toc" aria-label="目录">
        <template v-for="part in parts" :key="part.label">
          <h4>{{ part.label }}</h4>
          <a v-for="s in part.items" :key="s.id" :href="`#${s.id}`" :class="{ on: current === s.id }" @click.prevent="go(s.id)">{{ s.title }}</a>
        </template>
      </nav>
    </div>

    <div class="body">
      <nav v-if="hasToc" class="toc tocd desk-only" aria-label="目录">
        <template v-for="part in parts" :key="part.label">
          <h4>{{ part.label }}</h4>
          <a v-for="s in part.items" :key="s.id" :href="`#${s.id}`" :class="{ on: current === s.id }" :aria-current="current === s.id ? 'location' : undefined" @click.prevent="go(s.id)">{{ s.title }}</a>
        </template>
      </nav>
      <div class="content">
        <p v-if="paused" class="notice">目前不能通过 API 下单。</p>
        <!-- eslint-disable-next-line vue/no-v-html -- rendered at build time from our own Markdown, raw HTML off -->
        <div v-html="d.html" @click="onClick"></div>
      </div>
    </div>
    <SiteFooter class="dfoot" />
  </main>
</template>

<style>
/* the document's HTML comes from v-html: unscoped, under .docpage */
.docpage.withtoc { max-width: 1060px; }
.docpage .sec { scroll-margin-top: 56px; }
.docpage .sec h3 { font-size: 14px; font-weight: 700; margin: 14px 0 6px; }
.docpage .sec p, .docpage .lede p { font-size: 14px; line-height: 1.75; color: var(--ink-2); }
.docpage .sec > * + p, .docpage .sec > * + table, .docpage .sec > * + ul, .docpage .sec > * + pre, .docpage .sec > * + .formula { margin-top: 8px; }
.docpage .lede p + p { margin-top: 8px; }
.docpage .sec strong { color: var(--ink); }
.docpage .sec a, .docpage .lede a { color: var(--accent); }
.docpage .part { padding: 22px var(--gutter) 6px; font-size: 12px; color: var(--muted); letter-spacing: 0.04em; }
.docpage .part + .sec { border-top-width: 2px; }
.docpage .notice { padding: 12px var(--gutter) 0; font-size: 14px; color: var(--danger); }
.docpage code { font-family: var(--font-num); font-size: 0.92em; }
.docpage .code { padding: 10px 12px; background: var(--ground-2); overflow-x: auto; font-size: 12px; line-height: 1.6; color: var(--ink); }
.docpage .code code { font-size: inherit; white-space: pre; }
.docpage .formula { display: flex; flex-direction: column; font-size: 13px; line-height: 1.8; padding: 10px 12px; background: var(--ground-2); }
.docpage .formula .in { padding-left: 2em; }
.docpage .t { width: 100%; border-collapse: collapse; font-size: 13px; }
.docpage .t th { text-align: left; font-weight: 400; font-size: 12px; color: var(--muted); padding: 6px 12px 6px 0; border-bottom: 1px solid var(--hair); white-space: nowrap; }
.docpage .t td { padding: 9px 12px 9px 0; border-bottom: 1px solid var(--hair-2); vertical-align: top; line-height: 1.6; color: var(--ink-2); overflow-wrap: anywhere; }
.docpage .t td:first-child { white-space: nowrap; color: var(--ink); }
.docpage .t td.n { font-family: var(--font-num); font-variant-numeric: tabular-nums; white-space: nowrap; color: var(--ink); }

/* contents */
.docpage .toc h4 { margin: 10px 0 4px; font-size: 12px; font-weight: 400; color: var(--muted); }
.docpage .toc h4:first-child { margin-top: 0; }
.docpage .toc a { display: block; padding: 5px 0 5px 10px; font-size: 14px; color: var(--ink-2); border-left: 2px solid transparent; }
.docpage .toc a.on { color: var(--ink); font-weight: 700; border-left-color: var(--ink); }
.docpage .tocp { position: sticky; top: 0; z-index: 5; background: var(--ground); border-bottom: 1px solid var(--hair); }
.docpage .tocbtn { width: 100%; display: flex; justify-content: space-between; align-items: center; padding: 12px var(--gutter); border: 0; background: none; font-size: 14px; color: var(--ink); }
.docpage .tocbtn .cur { font-size: 13px; color: var(--muted); }
.docpage #toc-phone { max-height: 60vh; overflow-y: auto; padding: 0 var(--gutter) 12px; }
@media (min-width: 1024px) {
  .docpage.withtoc .body { display: grid; grid-template-columns: 200px minmax(0, 1fr); column-gap: 40px; padding: 0 32px; }
  .docpage .tocd { position: sticky; top: calc(var(--desk-header-h) + 20px); align-self: start; max-height: calc(100vh - var(--desk-header-h) - 40px); overflow-y: auto; padding-top: 22px; }
  .docpage.withtoc .content .sec, .docpage.withtoc .content .part, .docpage.withtoc .content .lede, .docpage.withtoc .content .notice { padding-left: 0; padding-right: 0; }
  .docpage.withtoc .dfoot { padding-left: 32px; padding-right: 32px; }
  .docpage .sec { scroll-margin-top: calc(var(--desk-header-h) + 12px); }
}
</style>
