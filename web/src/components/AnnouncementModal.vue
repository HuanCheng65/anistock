<script setup lang="ts">
// The operator's announcement (`anistock announce`): shown once after sign-in to every player who opened an
// account before it, the newest unread first. Dismissing it marks it read on the server, so it does not come
// back on another device; the next unread one, if any, follows. The body is Markdown (markdown.ts).
import { computed, ref, watch } from 'vue'
import { api, call } from '../api'
import { when } from '../format'
import { renderMarkdown } from '../markdown'
import { useSession } from '../stores/session'
import Modal from './Modal.vue'

const session = useSession()
const ann = computed(() => session.me?.announcement ?? null)
const busy = ref(false)
const html = ref('')
// the dialog opens once the body is rendered, so it does not grow under the reader
watch(() => ann.value?.body, async (b) => {
  html.value = ''
  if (!b) return
  const out = await renderMarkdown(b).catch(() => '')
  if (ann.value?.body === b) html.value = out || `<p>${b.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]!)}</p>`
}, { immediate: true })

async function dismiss() {
  const a = ann.value
  if (!a || busy.value) return
  busy.value = true
  try {
    session.me = await call(api.POST('/api/v1/me/announcements/{id}/read', { params: { path: { id: a.id } } }))
  } catch {
    // offline or the session ended: hide it for now, it comes back with the next /me
    if (session.me) session.me = { ...session.me, announcement: null }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <Modal v-if="ann && html" :open="true" :title="ann.title" :busy="busy" @close="dismiss">
    <p class="at">{{ when(ann.published_at) }}</p>
    <div class="md" v-html="html"></div>
    <template #actions>
      <button type="button" class="btn" :disabled="busy" @click="dismiss">知道了</button>
    </template>
  </Modal>
</template>

<style scoped>
.at { font-size: 12px; color: var(--muted); margin-bottom: 4px; }
.md { color: var(--ink); font-size: 14px; line-height: 1.75; overflow-wrap: anywhere; }
.md :deep(:is(p, ul, ol, blockquote, pre, table, hr, h1, h2, h3)) { margin: 0; }
.md :deep(:is(p, ul, ol, blockquote, pre, table, hr):not(:first-child)) { margin-top: 10px; }
.md :deep(h1), .md :deep(h2), .md :deep(h3) { font-family: var(--font-serif); font-weight: 900; line-height: 1.4; }
.md :deep(:is(h1, h2, h3):not(:first-child)) { margin-top: 16px; }
.md :deep(:is(h1, h2, h3) + *) { margin-top: 4px; }
.md :deep(h1) { font-size: 18px; }
.md :deep(h2) { font-size: 16px; }
.md :deep(h3) { font-size: 15px; }
.md :deep(ul), .md :deep(ol) { padding-left: 1.3em; }
.md :deep(ul) { list-style: disc; }
.md :deep(ol) { list-style: decimal; }
.md :deep(li + li) { margin-top: 4px; }
.md :deep(a) { color: var(--accent); text-decoration: underline; text-underline-offset: 3px; }
.md :deep(strong) { font-weight: 700; }
.md :deep(code) { font-family: var(--font-num); font-size: 13px; background: var(--ground-2); padding: 1px 4px; }
.md :deep(pre) { background: var(--ground-2); padding: 10px 12px; overflow-x: auto; }
.md :deep(pre code) { background: none; padding: 0; }
.md :deep(blockquote) { border-left: 2px solid var(--rule); padding-left: 12px; color: var(--ink-2); }
.md :deep(hr) { border: 0; border-top: 1px solid var(--hair); }
.md :deep(table) { border-collapse: collapse; width: 100%; font-size: 13px; }
.md :deep(th), .md :deep(td) { border-bottom: 1px solid var(--hair-2); padding: 6px 8px 6px 0; text-align: left; }
</style>
