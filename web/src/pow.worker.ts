// Solves a register / login proof of work off the main thread (docs/v2-account.md §3.2): find n in
// [0, max) with PBKDF2-SHA-256(password = String(n), salt = UTF-8 of salt, cost rounds, 32 bytes) = target.
type Job = { salt: string; cost: number; max: number; target: string }

const enc = new TextEncoder()
const BATCH = 8

async function hash(n: number, salt: BufferSource, cost: number): Promise<string> {
  const key = await crypto.subtle.importKey('raw', enc.encode(String(n)), 'PBKDF2', false, ['deriveBits'])
  const bits = await crypto.subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-256', salt, iterations: cost }, key, 256)
  return Array.from(new Uint8Array(bits), (b) => b.toString(16).padStart(2, '0')).join('')
}

self.onmessage = async (e: MessageEvent<Job>) => {
  const { salt, cost, max, target } = e.data
  const s = enc.encode(salt) as Uint8Array<ArrayBuffer>
  for (let n = 0; n < max; n += BATCH) {
    const ns: number[] = []
    for (let i = n; i < Math.min(n + BATCH, max); i++) ns.push(i)
    const hs = await Promise.all(ns.map((i) => hash(i, s, cost)))
    const k = hs.indexOf(target)
    if (k >= 0) return postMessage({ number: ns[k] })
  }
  postMessage({ number: null })
}
