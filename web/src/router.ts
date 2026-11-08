import { ref } from 'vue'
import { createRouter, createWebHistory, type RouteLocationNormalized } from 'vue-router'
import { useSession } from './stores/session'

/** a navigation is waiting (its view's chunk or the first /me) for longer than a moment: App.vue shows a bar */
export const navigating = ref(false)
/** the tab of the page being opened, highlighted at once (BottomNav) */
export const pendingTab = ref<string | null>(null)

const router = createRouter({
  history: createWebHistory(),
  // a sort or filter tap changes only the query: stay where the list is; a #section clears the sticky header or
  // contents bar above it
  scrollBehavior: (to, from, saved) => saved ?? (to.hash ? { el: to.hash, top: 72 } : to.path === from.path ? false : { top: 0 }),
  routes: [
    { path: '/', name: 'market', component: () => import('./views/MarketView.vue'), meta: { tab: 'market', title: '市场' } },
    { path: '/stocks', name: 'stocks', component: () => import('./views/StocksView.vue'), meta: { tab: 'stocks', title: '行情' } },
    { path: '/stocks/:id(\\d+)', name: 'stock', component: () => import('./views/StockView.vue'), meta: { tab: 'stocks', title: '股票', noNav: true } },
    { path: '/stock/:id', redirect: (to) => `/stocks/${to.params.id}` },
    { path: '/futures', name: 'futures', component: () => import('./views/FuturesListView.vue'), meta: { tab: 'stocks', title: '热度期货' } },
    { path: '/futures/:id(\\d+)', name: 'contract', component: () => import('./views/FuturesView.vue'), meta: { tab: 'stocks', title: '热度期货', noNav: true } },
    { path: '/funds', name: 'funds', component: () => import('./views/FundListView.vue'), meta: { tab: 'stocks', title: '基金' } },
    { path: '/funds/:key([a-z0-9_]+)', name: 'fund', component: () => import('./views/FundView.vue'), meta: { tab: 'stocks', title: '基金', noNav: true } },
    { path: '/watch', redirect: { path: '/stocks', query: { f: 'watch' } } },
    { path: '/ipo', name: 'ipo', component: () => import('./views/IpoView.vue'), meta: { tab: 'ipo', title: '新股认购' } },
    { path: '/news', name: 'news', component: () => import('./views/NewsView.vue'), meta: { tab: 'news', title: '资讯' } },
    { path: '/me', name: 'me', component: () => import('./views/PortfolioView.vue'), meta: { tab: 'me', title: '我的', auth: true } },
    { path: '/rank', name: 'rank', component: () => import('./views/RankView.vue'), meta: { tab: 'me', title: '排行' } },
    { path: '/macro', name: 'macro', component: () => import('./views/MacroView.vue'), meta: { tab: 'market', title: '宏观' } },
    { path: '/guide', name: 'guide', component: () => import('./views/GuideView.vue'), meta: { tab: 'me', title: '新手指南' } },
    { path: '/rules/api', name: 'api-doc', component: () => import('./views/DocView.vue'), props: { doc: 'api' }, meta: { tab: 'me', title: '开发者说明' } },
    { path: '/rules/heat-index', name: 'heat-method', component: () => import('./views/DocView.vue'), props: { doc: 'heat' }, meta: { tab: 'me', title: '指数编制方法' } },
    { path: '/rules', name: 'rules', component: () => import('./views/DocView.vue'), props: { doc: 'rules' }, meta: { tab: 'me', title: '规则' } },
    { path: '/settings', name: 'settings', component: () => import('./views/SettingsView.vue'), meta: { tab: 'me', title: '设置' } },
    { path: '/forgot', name: 'forgot', component: () => import('./views/ForgotView.vue'), meta: { title: '找回密码', bare: true } },
    { path: '/login', name: 'login', component: () => import('./views/LoginView.vue'), meta: { title: '登录', bare: true } },
    { path: '/assessment/:kind(risk|margin-quiz|futures-quiz)', name: 'assessment', component: () => import('./views/AssessmentView.vue'), meta: { tab: 'me', title: '测评', auth: true } },
    { path: '/legal/:doc', name: 'legal', component: () => import('./views/LegalView.vue'), meta: { tab: 'me', title: '协议' } },
    { path: '/welcome', name: 'welcome', component: () => import('./views/WelcomeView.vue'), meta: { title: '开户', bare: true, auth: true } },
    { path: '/more', redirect: '/me' },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

/** Start downloading the views of `to` now (the router would only after the guards). */
function preload(to: RouteLocationNormalized) {
  for (const r of to.matched) {
    const c = r.components?.default
    if (typeof c === 'function') Promise.resolve((c as () => unknown)()).catch(() => {})
  }
}

let slow: number | undefined
router.beforeEach(async (to) => {
  window.clearTimeout(slow)
  slow = window.setTimeout(() => (navigating.value = true), 120)
  pendingTab.value = (to.meta.tab as string | undefined) ?? null
  const s = useSession()
  if (!s.checked) {
    preload(to)
    try {
      await s.loadMe()
    } catch {
      /* network down: the page shows its own error */
    }
  }
  if (to.meta.auth && !s.me) return { name: 'login', query: { next: to.fullPath } }
})

function settled() {
  window.clearTimeout(slow)
  navigating.value = false
  pendingTab.value = null
}
router.afterEach((to) => {
  settled()
  document.title = to.name === 'market' ? 'AniEx 安易' : `${(to.meta.title as string) ?? ''} · AniEx 安易`
})

router.onError(settled)

// the main views, fetched while the browser is idle after the first page, so a tap does not wait for a chunk
const PREFETCH = ['stocks', 'stock', 'market', 'news', 'ipo', 'me']
export function prefetchViews() {
  const conn = (navigator as { connection?: { saveData?: boolean } }).connection
  if (conn?.saveData) return
  const idle = (f: () => void) =>
    typeof window.requestIdleCallback === 'function' ? window.requestIdleCallback(f, { timeout: 4000 }) : globalThis.setTimeout(f, 1500)
  const names = [...PREFETCH]
  const next = () => {
    const name = names.shift()
    if (!name) return
    const c = router.getRoutes().find((r) => r.name === name)?.components?.default
    Promise.resolve(typeof c === 'function' ? (c as () => unknown)() : null)
      .catch(() => {})
      .finally(() => idle(next))
  }
  idle(next)
}

export default router
