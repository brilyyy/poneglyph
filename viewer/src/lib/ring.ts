/** Append `item` to a fixed-capacity buffer, dropping oldest entries past
 *  `cap`. Returns a new array (immutable, so React sees a changed reference). */
export function pushCapped<T>(buf: readonly T[], item: T, cap: number): T[] {
  const next = [...buf, item]
  return next.length > cap ? next.slice(next.length - cap) : next
}
