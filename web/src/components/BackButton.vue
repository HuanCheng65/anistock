<script setup lang="ts">
import { useRouter } from 'vue-router'
import Icon from './Icon.vue'
const props = defineProps<{ fallback?: string }>()
const router = useRouter()
function back() {
  if (window.history.state?.back) router.back()
  else router.push(props.fallback ?? '/me')
}
</script>

<template>
  <button type="button" class="iconbtn" aria-label="返回" @click="back"><Icon name="back" /></button>
</template>
