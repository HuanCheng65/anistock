import { createRouter, createWebHistory } from 'vue-router'
import { useSession } from './stores/session'

const router = createRouter({
  history: createWebHistory(),
  scrollBehavior: (_to, _from, saved) => saved ?? { top: 0 },
  routes: [
    { path: '/', name: 'market', component: () => import('./views/MarketView.vue'), meta: { tab: 'market', title: '行情' } },
    { path: '/stock/:id', name: 'stock', component: () => import('./views/StockView.vue'), meta: { tab: 'market', title: '个股' } },
    { path: '/ipo', name: 'ipo', component: () => import('./views/IpoView.vue'), meta: { tab: 'market', title: '新股认购' } },
    { path: '/me', name: 'me', component: () => import('./views/PortfolioView.vue'), meta: { tab: 'me', title: '我的', auth: true } },
    { path: '/rank', name: 'rank', component: () => import('./views/LeaderboardView.vue'), meta: { tab: 'rank', title: '排行' } },
    { path: '/news', name: 'news', component: () => import('./views/NewsView.vue'), meta: { tab: 'news', title: '新闻' } },
    { path: '/more', name: 'more', component: () => import('./views/MoreView.vue'), meta: { tab: 'more', title: '更多' } },
    { path: '/guide', name: 'guide', component: () => import('./views/GuideView.vue'), meta: { tab: 'more', title: '新手指南' } },
    { path: '/rules', name: 'rules', component: () => import('./views/RulesView.vue'), meta: { tab: 'more', title: '规则' } },
    { path: '/macro', name: 'macro', component: () => import('./views/MacroView.vue'), meta: { tab: 'more', title: '宏观' } },
    { path: '/login', name: 'login', component: () => import('./views/LoginView.vue'), meta: { title: '登录' } },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

router.beforeEach(async (to) => {
  const s = useSession()
  if (!s.checked) {
    try {
      await s.loadMe()
    } catch {
      /* network down: let the page show its own error */
    }
  }
  if (to.meta.auth && !s.me) return { name: 'login', query: { next: to.fullPath } }
})

router.afterEach((to) => {
  document.title = `${(to.meta.title as string) ?? ''} · AniStock`
})

export default router
