// Frontend DSP mirror of `core::dsp` (ADR-015) used to preview filtered traces
// next to the raw signal. The authoritative, tested implementations live in
// Rust; this small mirror keeps the filter UI live without per-sample IPC.
// Add an algorithm here and to `core::dsp::registry` together.

export type AlgoKind = 'processor' | 'analyzer'
export interface ParamSpec {
  key: string
  label: string
  unit: string | null
  kind: { kind: 'Float'; min: number; max: number; step: number; log: boolean } | { kind: 'Int'; min: number; max: number } | { kind: 'Enum'; options: string[] } | { kind: 'Bool' }
  default: number
}
export interface AlgorithmInfo {
  id: string
  name: string
  kind: AlgoKind
  params: ParamSpec[]
}

/** A single-sample streaming filter. */
export interface Processor {
  configure(p: Record<string, number>, fs: number): void
  reset(): void
  process(x: number): number
}

/** Direct Form II Transposed biquad (low/high pass). */
export class Biquad implements Processor {
  private kind: 'lowpass' | 'highpass'
  private fc = 5
  private q = 1 / Math.sqrt(2)
  private fs = 100
  private b0 = 0
  private b1 = 0
  private b2 = 0
  private a1 = 0
  private a2 = 0
  private x1 = 0
  private x2 = 0
  constructor(kind: 'lowpass' | 'highpass', fs: number) {
    this.kind = kind
    this.fs = fs
  }
  configure(p: Record<string, number>, fs: number): void {
    this.fs = fs
    this.fc = p.fc_hz ?? this.fc
    this.q = p.q ?? this.q
    const k = Math.tan((Math.PI * this.fc) / this.fs)
    const norm = 1 / (1 + k / this.q + k * k)
    if (this.kind === 'lowpass') {
      this.b0 = k * k * norm
      this.b1 = 2 * this.b0
      this.b2 = this.b0
    } else {
      this.b0 = norm
      this.b1 = -2 * norm
      this.b2 = norm
    }
    this.a1 = 2 * (k * k - 1) * norm
    this.a2 = (1 - k / this.q + k * k) * norm
  }
  reset(): void {
    this.x1 = 0
    this.x2 = 0
  }
  process(x: number): number {
    const y = this.b0 * x + this.x1
    this.x1 = this.b1 * x - this.a1 * y + this.x2
    this.x2 = this.b2 * x - this.a2 * y
    return y
  }
}

/** Causal moving average (ring buffer + running sum). */
export class MovingAverage implements Processor {
  private window = 10
  private buf: number[] = []
  private sum = 0
  configure(p: Record<string, number>): void {
    this.window = Math.max(1, Math.round(p.window ?? 10))
    this.buf = []
    this.sum = 0
  }
  reset(): void {
    this.buf = []
    this.sum = 0
  }
  process(x: number): number {
    if (this.buf.length === this.window) this.sum -= this.buf.shift() ?? 0
    this.buf.push(x)
    this.sum += x
    return this.sum / this.buf.length
  }
}

/** Sliding-window mean/linear detrend (running sums). */
export class Detrend implements Processor {
  private window = 100
  private linear = true
  private ts: number[] = []
  private xs: number[] = []
  private sX = 0
  private sT = 0
  private sT2 = 0
  private sTX = 0
  private n = 0
  configure(p: Record<string, number>): void {
    this.window = Math.max(2, Math.round(p.window ?? 100))
    this.linear = (p.mode ?? 1) !== 0
    this.reset()
  }
  reset(): void {
    this.ts = []
    this.xs = []
    this.sX = 0
    this.sT = 0
    this.sT2 = 0
    this.sTX = 0
    this.n = 0
  }
  process(x: number): number {
    const t = this.n++
    const fitted = this.fit(t)
    if (this.ts.length === this.window) {
      const ot = this.ts.shift() ?? 0
      const ox = this.xs.shift() ?? 0
      this.sX -= ox
      this.sT -= ot
      this.sT2 -= ot * ot
      this.sTX -= ot * ox
    }
    this.ts.push(t)
    this.xs.push(x)
    this.sX += x
    this.sT += t
    this.sT2 += t * t
    this.sTX += t * x
    return x - fitted
  }
  private fit(t: number): number {
    const n = this.xs.length
    if (n < 2) return n === 0 ? 0 : this.sX / n
    if (!this.linear) return this.sX / n
    const denom = n * this.sT2 - this.sT * this.sT
    const slope = Math.abs(denom) > 1e-15 ? (n * this.sTX - this.sT * this.sX) / denom : 0
    const intercept = (this.sX - slope * this.sT) / n
    return slope * t + intercept
  }
}

/** Build a processor from the core registry's algorithm id. */
export function makeProcessor(id: string, p: Record<string, number>, fs: number): Processor {
  switch (id) {
    case 'lpf2':
      return configure(new Biquad('lowpass', fs), p, fs)
    case 'hpf2':
      return configure(new Biquad('highpass', fs), p, fs)
    case 'moving_average':
      return configure(new MovingAverage(), p, fs)
    case 'detrend':
      return configure(new Detrend(), p, fs)
    default:
      throw new Error(`unknown filter algorithm ${id}`)
  }
}
function configure(proc: Processor, p: Record<string, number>, fs: number): Processor {
  proc.configure(p, fs)
  return proc
}

/** In-place radix-2 FFT (mirror of core::dsp::fft). */
export function fftInplace(re: number[], im: number[]): void {
  const n = re.length
  let j = 0
  for (let i = 1; i < n; i++) {
    let bit = n >> 1
    while (j & bit) {
      j ^= bit
      bit >>= 1
    }
    j ^= bit
    if (i < j) {
      ;[re[i], re[j]] = [re[j], re[i]]
      ;[im[i], im[j]] = [im[j], im[i]]
    }
  }
  let len = 2
  while (len <= n) {
    const angle = (-2 * Math.PI) / len
    const twR = Math.cos(angle)
    const twI = Math.sin(angle)
    const half = len >> 1
    for (let start = 0; start < n; start += len) {
      let cr = 1
      let ci = 0
      let a = start
      let b = start + half
      for (let u = 0; u < half; u++) {
        const xr = re[b] * cr - im[b] * ci
        const xi = im[b] * cr + re[b] * ci
        re[b] = re[a] - xr
        im[b] = im[a] - xi
        re[a] += xr
        im[a] += xi
        const tr = cr * twR - ci * twI
        ci = ci * twR + cr * twI
        cr = tr
        a++
        b++
      }
    }
    len <<= 1
  }
}

export interface Spectrum {
  n: number
  fs: number
  deltaF: number
  nyquist: number
  bins: number[]
  peakBin: number
  peakFreqHz: number
  peakValue: number
}

/** Magnitude spectrum of the last `n` samples (rect window). */
export function magnitudeSpectrum(x: number[], fs: number): Spectrum | null {
  const n = x.length
  if (n < 2 || (n & (n - 1)) !== 0) return null
  const re = x.slice()
  const im = new Array<number>(n).fill(0)
  fftInplace(re, im)
  let peakBin = 0
  let peakValue = -Infinity
  const bins = new Array<number>(n)
  for (let i = 0; i < n; i++) {
    const v = Math.hypot(re[i], im[i]) / (n / 2)
    bins[i] = v
    if (i > 0 && v > peakValue) {
      peakValue = v
      peakBin = i
    }
  }
  return {
    n,
    fs,
    deltaF: fs / n,
    nyquist: fs / 2,
    bins,
    peakBin,
    peakFreqHz: (peakBin * fs) / n,
    peakValue,
  }
}