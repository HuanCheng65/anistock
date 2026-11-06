<script setup lang="ts">
// 融资融券 on 我的: a quiet entry with an in-page confirmation while credit is not open; once open, the
// maintenance ratio, what is owed per stock, and direct repayment.
import { computed, onMounted, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { errorText, type Me } from '../api'
import { money, qty, share, toCents } from '../format'
import { useCredit } from '../stores/credit'
import { markEditing } from '../version'

const props = defineProps<{ me: Me }>()
const credit = useCredit()
const opening = ref(false)
const repaying = ref(false)
const amount = ref('')
const busy = ref(false)
const error = ref('')
const done = ref('')

onMounted(() => credit.refresh().catch(() => {}))
watch(() => props.me.credit_status, () => credit.refresh().catch(() => {}))
watch([opening, repaying], ([a, b]) => markEditing('credit', a || b))

const c = computed(() => credit.account)
const rate = (v: number) => share(v, 2)
const low = computed(() => c.value?.maintenance_ratio != null && c.value.maintenance_ratio < c.value.call_ratio)
const owed = computed(() => (c.value ? c.value.debt + c.value.interest : 0))

async function open() {
  busy.value = true
  error.value = ''
  try {
    await credit.open()
    opening.value = false
  } catch (e) {
    error.value = errorText(e, '无法开通，稍后重试')
  } finally {
    busy.value = false
  }
}

async function repay() {
  const cents = amount.value.trim() ? toCents(amount.value) : null
  if (amount.value.trim() && !(cents && cents > 0)) {
    error.value = '金额需大于 0，最多两位小数'
    return
  }
  busy.value = true
  error.value = ''
  try {
    const paid = await credit.repay(cents)
    done.value = `已还款 ${money(paid)} 星辉`
    repaying.value = false
    amount.value = ''
  } catch (e) {
    error.value = errorText(e, '无法还款，稍后重试')
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section v-if="c" class="cr gutter">
    <template v-if="!c.opened">
      <button v-if="!opening" type="button" class="entry" @click="opening = true">
        <span>融资融券</span>
        <span class="hint">{{ c.can_open ? '未开通' : c.open_message }}</span>
      </button>
      <div v-else class="confirm" role="group" aria-labelledby="cr-title">
        <h2 id="cr-title">开通融资融券？</h2>
        <p>现金和全部持仓作为担保物。</p>
        <p>融资利率 {{ rate(c.financing_rate) }}，融券费率 {{ rate(c.lending_rate) }}，按日计息。</p>
        <p>维持担保比例低于 {{ share(c.call_ratio) }} 需追加担保物，低于 {{ share(c.liquidate_ratio) }} 强制平仓。</p>
        <p>穿仓后排行的本段收益记为 −100%。</p>
        <p v-if="error" class="msg error" role="alert">{{ error }}</p>
        <p v-else-if="!c.can_open && c.open_message" class="wait">{{ c.open_message }}</p>
        <div class="acts">
          <button type="button" class="btn small" :disabled="busy || !c.can_open" @click="open">{{ busy ? '处理中' : '开通' }}</button>
          <button type="button" class="btn small ghost" :disabled="busy" @click="opening = false; error = ''">取消</button>
        </div>
      </div>
    </template>

    <template v-else>
      <h2 class="head">融资融券</h2>
      <dl class="facts">
        <div><dt>维持担保比例</dt><dd :class="{ warn: low }">{{ c.maintenance_ratio != null ? share(c.maintenance_ratio) : '–' }}</dd></div>
        <div><dt>负债</dt><dd>{{ money(c.liabilities) }}</dd></div>
        <div><dt>可用保证金</dt><dd>{{ money(c.available_margin) }}</dd></div>
      </dl>
      <ul v-if="c.debts.length || c.shorts.length" class="owe">
        <li v-for="d in c.debts" :key="'d' + d.stock.id">
          <RouterLink :to="`/stocks/${d.stock.id}`" class="serif clamp1">{{ d.stock.title }}</RouterLink>
          <span class="k">融资</span>
          <span class="num">{{ money(d.principal + d.interest) }}</span>
        </li>
        <li v-for="s in c.shorts" :key="'s' + s.stock.id">
          <RouterLink :to="`/stocks/${s.stock.id}`" class="serif clamp1">{{ s.stock.title }}</RouterLink>
          <span class="k">融券 <span class="num">{{ qty(s.quantity) }}</span> 股</span>
          <span class="num">{{ money(s.value + s.fee) }}</span>
        </li>
      </ul>
      <p class="rates">融资利率 {{ rate(c.financing_rate) }} · 融券费率 {{ rate(c.lending_rate) }}</p>
      <template v-if="owed > 0 && c.status !== 'defaulted'">
        <button v-if="!repaying" type="button" class="linkbtn" @click="repaying = true; done = ''">直接还款</button>
        <div v-else class="repay">
          <input v-model="amount" class="input num" inputmode="decimal" autocomplete="off" aria-label="还款金额"
                 :placeholder="`全部 ${money(owed)}`" />
          <button type="button" class="btn small" :disabled="busy" @click="repay">{{ busy ? '处理中' : '还款' }}</button>
          <button type="button" class="btn small ghost" :disabled="busy" @click="repaying = false; error = ''">取消</button>
        </div>
      </template>
      <p v-if="error" class="msg error" role="alert">{{ error }}</p>
      <p v-else-if="done" class="msg ok" role="status">{{ done }}</p>
    </template>
  </section>
</template>

<style scoped>
.cr { border-bottom: 1px solid var(--hair-2); padding-bottom: 12px; }
.entry { display: flex; align-items: baseline; gap: 12px; width: 100%; padding: 12px 0 0; border: 0; background: none; color: var(--ink-2); font-size: 14px; text-align: left; }
.hint { margin-left: auto; font-size: 12px; color: var(--muted); text-align: right; }
.confirm { padding: 14px 0 0; display: flex; flex-direction: column; gap: 6px; }
.confirm h2, .head { margin: 0 0 4px; font-size: 15px; font-weight: 700; color: var(--ink); }
.head { padding-top: 14px; }
.confirm p { margin: 0; font-size: 13px; line-height: 1.6; }
.confirm p:not(.msg) { color: var(--ink-2); }
.confirm .wait { color: var(--muted); }
.acts { display: flex; gap: 8px; margin-top: 8px; }
.facts dd.warn { color: var(--danger); }
.owe { list-style: none; margin: 4px 0 0; padding: 0; border-top: 1px solid var(--hair-2); }
.owe li { display: grid; grid-template-columns: minmax(0, 1fr) auto auto; gap: 10px; align-items: baseline; padding: 8px 0; border-bottom: 1px solid var(--hair-2); font-size: 13px; }
.owe a { color: var(--ink); font-weight: 600; min-width: 0; }
.k { font-size: 12px; color: var(--muted); }
.rates { margin: 8px 0 6px; font-size: 12px; color: var(--muted); }
.repay { display: grid; grid-template-columns: minmax(0, 1fr) auto auto; gap: 8px; margin-top: 4px; }
.repay .input { height: 32px; min-width: 0; }
.msg { margin-top: 6px; }
</style>
