import { createRouter, createWebHistory } from 'vue-router'
import { useSession } from './stores/session'

const router = createRouter({
  history: createWebHistory(),
  scrollBehavior: (_to, _from, saved) => saved ?? { top: 0 },
  routes: [
    { path: '/', name: 'market', component: () => import('./views/MarketView.vue'), meta: { tab: 'market', title: '市场' } },
    { path: '/stocks', name: 'stocks', component: () => import('./views/StocksView.vue'), meta: { tab: 'stocks', title: '股票' } },
    { path: '/stocks/:id(\\d+)', name: 'stock', component: () => import('./views/StockView.vue'), meta: { tab: 'stocks', title: '股票', noNav: true } },
    { path: '/stock/:id', redirect: (to) => `/stocks/${to.params.id}` },
    { path: '/watch', redirect: { path: '/stocks', query: { f: 'watch' } } },
    { path: '/ipo', name: 'ipo', component: () => import('./views/IpoView.vue'), meta: { tab: 'ipo', title: '新股认购' } },
    { path: '/news', name: 'news', component: () => import('./views/NewsView.vue'), meta: { tab: 'news', title: '资讯' } },
    { path: '/me', name: 'me', component: () => import('./views/PortfolioView.vue'), meta: { tab: 'me', title: '我的', auth: true } },
    { path: '/rank', name: 'rank', component: () => import('./views/RankView.vue'), meta: { tab: 'me', title: '排行' } },
    { path: '/macro', name: 'macro', component: () => import('./views/MacroView.vue'), meta: { tab: 'market', title: '宏观' } },
    { path: '/guide', name: 'guide', component: () => import('./views/GuideView.vue'), meta: { tab: 'me', title: '新手指南' } },
    { path: '/rules', name: 'rules', component: () => import('./views/RulesView.vue'), meta: { tab: 'me', title: '规则' } },
    { path: '/settings', name: 'settings', component: () => import('./views/SettingsView.vue'), meta: { tab: 'me', title: '设置' } },
    { path: '/login', name: 'login', component: () => import('./views/LoginView.vue'), meta: { title: '登录', bare: true } },
    { path: '/welcome', name: 'welcome', component: () => import('./views/WelcomeView.vue'), meta: { title: '新手引导', bare: true, auth: true } },
    { path: '/more', redirect: '/me' },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

router.beforeEach(async (to) => {
  const s = useSession()
  if (!s.checked) {
    try {
      await s.loadMe()
    } catch {
      /* network down: the page shows its own error */
    }
  }
  if (to.meta.auth && !s.me) return { name: 'login', query: { next: to.fullPath } }
})

router.afterEach((to) => {
  document.title = to.name === 'market' ? 'AniEx 安易' : `${(to.meta.title as string) ?? ''} · AniEx 安易`
})

export default router
