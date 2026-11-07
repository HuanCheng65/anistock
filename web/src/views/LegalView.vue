<script setup lang="ts">
// /legal/:doc — 用户协议, 隐私政策, 风险揭示书, 融资融券风险揭示书, 程序化交易风险揭示书, readable without signing in.
// With ?sign=1 a document signed by typing the username shows the signature field at the end.
import { computed, ref, watchEffect } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { api, call, errorText } from '../api'
import BackButton from '../components/BackButton.vue'
import LegalText from '../components/LegalText.vue'
import { DOCUMENTS, SIGNED, isDocKey } from '../legal/documents'
import { useSession } from '../stores/session'

const route = useRoute()
const router = useRouter()
const session = useSession()
const key = computed(() => (isDocKey(route.params.doc) ? route.params.doc : null))
const doc = computed(() => (key.value ? DOCUMENTS[key.value] : null))
watchEffect(() => {
  if (doc.value) document.title = `${doc.value.title} · AniEx 安易`
})

// the version to sign, while it is still unsigned
const version = computed<number | null>(() => {
  const me = session.me
  if (!me || !key.value || !SIGNED.includes(key.value)) return null
  if (key.value === 'margin_disclosure') return me.credit_steps.disclosure_signed ? null : me.credit_steps.disclosure_version
  if (key.value === 'api_disclosure') return me.api_trading.disclosure_signed ? null : me.api_trading.disclosure_version
  return me.pending_documents.find((d) => d.doc === key.value)?.version ?? null
})
const signing = computed(() => route.query.sign === '1' && version.value != null)
const signature = ref('')
const matches = computed(() => signature.value.trim() === session.me?.username)
const busy = ref(false)
const error = ref('')
async function sign() {
  if (!matches.value || version.value == null || !key.value) return
  busy.value = true
  error.value = ''
  try {
    session.me = await call(api.POST('/api/v1/me/agreements', { body: { doc: key.value, version: version.value, signature: signature.value.trim() } }))
    if (window.history.state?.back) router.back()
    else router.replace('/me')
  } catch (e) {
    error.value = errorText(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <main class="page doc">
    <header class="pagehead back">
      <BackButton />
      <h1>{{ doc?.title ?? '文件不存在' }}</h1>
    </header>
    <div v-if="doc" class="body gutter">
      <LegalText :doc="doc" />
      <form v-if="signing" class="sign" @submit.prevent="sign">
        <label class="field">签名
          <input v-model="signature" class="input" autocomplete="off" autocapitalize="off" spellcheck="false" :placeholder="session.me?.username" />
        </label>
        <p v-if="error" class="msg error" role="alert">{{ error }}</p>
        <button type="submit" class="btn block" :disabled="busy || !matches">签署</button>
      </form>
    </div>
  </main>
</template>

<style scoped>
.body { padding-top: 16px; padding-bottom: 24px; }
.sign { display: flex; flex-direction: column; gap: 12px; margin-top: 20px; padding-top: 18px; border-top: 1px solid var(--rule); }
.sign .btn { height: 48px; font-size: 16px; }
</style>
