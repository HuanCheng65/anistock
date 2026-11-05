# 安易 AniEx 前端

Vue 3 + Vite + TypeScript，vue-router、Pinia，图表用 lightweight-charts 4.1.3（打包进产物，不依赖 CDN）。

```bash
npm ci            # 国内慢的话：npm ci --registry=https://registry.npmmirror.com
npm run dev       # 开发：/api 代理到 http://127.0.0.1:8080
npm run build     # 产物在 web/dist，FastAPI 会直接托管（先 build 再启动服务）
```

页面：行情、个股（K 线 / 五档盘口 / 下单 / 基本面）、我的、排行、新闻、更多（新手指南、规则、宏观）。
接口类型集中在 `src/api.ts`，与 `server/app.py` 对应。
