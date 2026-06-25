import { describe, expect, it } from 'vitest'

import { pushCapped } from './ring.ts'

describe('pushCapped', () => {
  it('appends below capacity and returns a new array', () => {
    const a = [1, 2]
    const b = pushCapped(a, 3, 5)
    expect(b).toEqual([1, 2, 3])
    expect(b).not.toBe(a) // new reference for React
  })

  it('drops the oldest entries once over capacity', () => {
    let buf: number[] = []
    for (let i = 1; i <= 7; i++) buf = pushCapped(buf, i, 3)
    expect(buf).toEqual([5, 6, 7])
  })
})
