import '@fontsource-variable/noto-serif-sc/wght.css'
import '@fontsource/ibm-plex-mono/latin-400.css'
import '@fontsource/ibm-plex-mono/latin-500.css'
import '@fontsource/ibm-plex-mono/latin-600.css'
import './styles/tokens.css'
import './styles/base.css'

import { createPinia } from 'pinia'
import { createApp } from 'vue'
import App from './App.vue'
import router from './router'
import { vFlash } from './flash'

createApp(App).use(createPinia()).use(router).directive('flash', vFlash).mount('#app')
