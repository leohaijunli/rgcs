// A preallocated three-channel ring buffer (time, raw, filtered) for one trace.
// Replaces the old `number[]` + `splice` + `Float64Array.from` copy per frame:
// pushes are O(1), time-based trimming is O(dropped), and memory is bounded by
// `cap` (S4: "预分配 Float64Array 环形缓冲").

export class Ring {
  private t: Float64Array
  private raw: Float64Array
  private filt: Float64Array
  private start = 0
  private size = 0

  constructor(public readonly cap: number) {
    this.t = new Float64Array(cap)
    this.raw = new Float64Array(cap)
    this.filt = new Float64Array(cap)
  }

  get length(): number {
    return this.size
  }

  push(t: number, raw: number, filt: number): void {
    const i = (this.start + this.size) % this.cap
    this.t[i] = t
    this.raw[i] = raw
    this.filt[i] = filt
    if (this.size < this.cap) this.size++
    else this.start = (this.start + 1) % this.cap
  }

  clear(): void {
    this.start = 0
    this.size = 0
  }

  timeAt(i: number): number {
    return this.t[(this.start + i) % this.cap]
  }
  rawAt(i: number): number {
    return this.raw[(this.start + i) % this.cap]
  }
  filtAt(i: number): number {
    return this.filt[(this.start + i) % this.cap]
  }

  newestTime(): number {
    return this.size === 0 ? 0 : this.timeAt(this.size - 1)
  }

  /** Index of the first sample with time >= cutoff (times are increasing). */
  firstAtOrAfter(cutoff: number): number {
    let lo = 0
    let hi = this.size
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if (this.timeAt(mid) < cutoff) lo = mid + 1
      else hi = mid
    }
    return lo
  }

  /** Drop samples older than `cutoff`, but never below `minKeep` samples. */
  trimBefore(cutoff: number, minKeep: number): void {
    const maxDrop = Math.max(0, this.size - minKeep)
    const drop = Math.min(this.firstAtOrAfter(cutoff), maxDrop)
    this.start = (this.start + drop) % this.cap
    this.size -= drop
  }
}
