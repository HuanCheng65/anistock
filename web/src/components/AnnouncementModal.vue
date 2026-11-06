<script setup lang="ts">
// The operator's announcement (`anistock announce`): shown once after sign-in to every player who opened an
// account before it, the newest unread first. Dismissing it marks it read on the server, so it does not come
// back on another device; the next unread one, if any, follows.
import { computed, ref } from 'vue'
import { api, call } from '../api'
import { when } from '../format'
import { useSession } from '../stores/session'
import Modal from './Modal.vue'

const session = useSession()
const ann = computed(() => session.me?.announcement ?? null)
const busy = ref(false)
const paragraphs = computed(() => (ann.value?.body ?? '').split(/\n\s*\n/).map((p) => p.trim()).filter(Boolean))

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
  <Modal v-if="ann" :open="true" :title="ann.title" :busy="busy" @close="dismiss">
    <p class="at">{{ when(ann.published_at) }}</p>
    <p v-for="(p, i) in paragraphs" :key="i" class="para">{{ p }}</p>
    <template #actions>
      <button type="button" class="btn" :disabled="busy" @click="dismiss">知道了</button>
    </template>
  </Modal>
</template>

<style scoped>
.at { font-size: 12px; color: var(--muted); margin-bottom: 4px; }
.para { white-space: pre-line; color: var(--ink); }
</style>
