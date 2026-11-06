<script setup lang="ts">
// A dialog over the page: a bottom sheet on phones, a centred card on wider screens. Native <dialog> with
// showModal() gives the focus trap, Esc and the inert page for free. Copy rules for confirmations
// (docs/copywriting.md): a question as the title, the main button repeating the action.
// `editKey` holds a pending reload (version.ts) while the dialog is open, like the in-page forms did.
import { nextTick, onBeforeUnmount, onMounted, ref, useId, watch } from 'vue'
import { markEditing } from '../version'

const props = defineProps<{ open: boolean; title: string; editKey?: string; busy?: boolean }>()
const emit = defineEmits<{ close: [] }>()
const el = ref<HTMLDialogElement | null>(null)
const sheet = ref<HTMLElement | null>(null)
const titleId = `dlg-${useId()}`

async function sync(open: boolean) {
  if (props.editKey) markEditing(props.editKey, open)
  await nextTick()
  const d = el.value
  if (!d) return
  if (open && !d.open) {
    d.showModal()
    // never the main button first (Enter would confirm at once): an input marked autofocus, else the sheet
    ;(d.querySelector<HTMLElement>('.content [autofocus]') ?? sheet.value)?.focus()
  } else if (!open && d.open) d.close()
}
onMounted(() => sync(props.open))
watch(() => props.open, sync)
onBeforeUnmount(() => {
  if (props.editKey) markEditing(props.editKey, false)
  if (el.value?.open) el.value.close()
})

function dismiss() {
  if (!props.busy) emit('close')
}
// Esc: the dialog closes only through `open`, so the parent's state stays the truth
function onCancel(e: Event) {
  e.preventDefault()
  dismiss()
}
// closed natively anyway (a repeated Esc is not cancelable): the parent's state follows the element
function onClosed() {
  if (props.open) emit('close')
}
// a press on the backdrop lands on the <dialog> itself; the content fills it edge to edge
function onClick(e: MouseEvent) {
  if (e.target === el.value) dismiss()
}
</script>

<template>
  <Teleport to="body">
    <dialog ref="el" class="modal" :aria-labelledby="titleId" @cancel="onCancel" @close="onClosed" @click="onClick">
      <div ref="sheet" class="sheet" tabindex="-1">
        <h2 :id="titleId" class="title">{{ title }}</h2>
        <div class="content"><slot /></div>
        <div v-if="$slots.actions" class="actions"><slot name="actions" /></div>
      </div>
    </dialog>
  </Teleport>
</template>

<style scoped>
.modal {
  /* phones: a sheet along the bottom edge */
  position: fixed; inset: auto 0 0 0; margin: 0; width: 100%; max-width: 100%; max-height: 88vh; max-height: 88dvh;
  padding: 0; border: 0; border-top: 2px solid var(--rule); background: var(--ground); color: var(--ink);
  overflow: hidden;
}
.modal[open] { animation: rise 0.24s cubic-bezier(0.2, 0.7, 0.2, 1); }
.modal::backdrop { background: rgb(0 0 0 / 0.42); }
.modal[open]::backdrop { animation: fade 0.24s ease; }
.sheet {
  outline: none; display: flex; flex-direction: column; max-height: inherit;
  padding: 18px 20px calc(18px + env(safe-area-inset-bottom, 0px));
}
.title { margin: 0 0 10px; font-family: var(--font-serif); font-size: 19px; font-weight: 900; line-height: 1.35; }
.content { min-height: 0; overflow-y: auto; overscroll-behavior: contain; display: flex; flex-direction: column; gap: 6px; font-size: 14px; line-height: 1.65; color: var(--ink-2); }
.content :deep(p) { margin: 0; }
.actions { display: flex; flex-direction: row-reverse; gap: 8px; padding-top: 16px; }
.actions :deep(.btn) { flex: 1; height: 44px; }

@media (min-width: 640px) {
  .modal { inset: 0; margin: auto; width: min(440px, calc(100% - 32px)); height: fit-content; max-height: 80vh; border: 1px solid var(--base-line); border-top: 2px solid var(--rule); }
  .modal[open] { animation-name: pop; }
  .sheet { padding: 22px 24px 20px; }
  .actions :deep(.btn) { flex: none; min-width: 96px; }
}

@keyframes rise { from { transform: translateY(24px); opacity: 0; } }
@keyframes pop { from { transform: translateY(8px); opacity: 0; } }
@keyframes fade { from { opacity: 0; } }
@media (prefers-reduced-motion: reduce) {
  .modal[open], .modal[open]::backdrop { animation: none; }
}
</style>

<style>
/* the page behind an open dialog does not scroll */
html:has(dialog.modal[open]) { overflow: hidden; }
</style>
