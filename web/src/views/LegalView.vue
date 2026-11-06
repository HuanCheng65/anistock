<script setup lang="ts">
// /legal/:doc — 用户协议, 隐私政策, 风险揭示书, readable without signing in.
import { computed, watchEffect } from 'vue'
import { useRoute } from 'vue-router'
import BackButton from '../components/BackButton.vue'
import LegalText from '../components/LegalText.vue'
import { DOCUMENTS, isDocKey } from '../legal/documents'

const route = useRoute()
const doc = computed(() => (isDocKey(route.params.doc) ? DOCUMENTS[route.params.doc] : null))
watchEffect(() => {
  if (doc.value) document.title = `${doc.value.title} · AniEx 安易`
})
</script>

<template>
  <main class="page doc">
    <header class="pagehead back">
      <BackButton />
      <h1>{{ doc?.title ?? '文件不存在' }}</h1>
    </header>
    <div v-if="doc" class="body gutter">
      <LegalText :doc="doc" />
    </div>
  </main>
</template>

<style scoped>
.body { padding-top: 16px; padding-bottom: 24px; }
</style>
