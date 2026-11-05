<script setup lang="ts">
// A new server version while the player is entering an order or has a dialog open (version.ts).
import { reloadNow, updateReady } from '../version'
</script>

<template>
  <div v-if="updateReady" class="upd" role="status">
    <span>有新版本，刷新后可用</span>
    <button type="button" class="btn small" @click="reloadNow">刷新</button>
  </div>
</template>

<style scoped>
.upd {
  position: fixed; z-index: 70; top: 0; left: 0; right: 0; display: flex; align-items: center; justify-content: center; gap: 14px;
  padding: 8px var(--gutter); background: var(--ink); color: var(--ground); font-size: 13px;
}
.upd .btn { background: var(--ground); color: var(--ink); border-color: var(--ground); }
</style>
