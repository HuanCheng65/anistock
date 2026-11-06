// The register / login proof of work (docs/v2-account.md §3.2): solved in a Web Worker with crypto.subtle.
import type { components } from './api/schema'

export type PowChallenge = components['schemas']['PowChallenge']
export type PowSolution = components['schemas']['PowSolution']

/** Some in-app browsers lack crypto.subtle or module workers. */
export function powSupported(): boolean {
  return typeof Worker !== 'undefined' && typeof crypto !== 'undefined' && !!crypto.subtle
}

export function solve(c: PowChallenge): Promise<PowSolution> {
  return new Promise((resolve, reject) => {
    const w = new Worker(new URL('./pow.worker.ts', import.meta.url), { type: 'module' })
    w.onmessage = (e: MessageEvent<{ number: number | null }>) => {
      w.terminate()
      if (e.data.number == null) reject(new Error('no answer'))
      else resolve({ challenge: c, number: e.data.number })
    }
    w.onerror = () => {
      w.terminate()
      reject(new Error('worker failed'))
    }
    w.postMessage({ salt: c.salt, cost: c.cost, max: c.max, target: c.target })
  })
}
