import { X } from 'lucide-react'
import { useLinkStore } from '../stores/link'

/** Transient error banner (link failures, endpoint errors, etc.). */
export default function ErrorBanner() {
  const lastError = useLinkStore((s) => s.lastError)
  const setError = useLinkStore((s) => s.setError)
  if (!lastError) return null

  return (
    <div className="absolute bottom-3 right-3 z-20 flex max-w-sm items-center gap-3 rounded border border-line bg-panel/95 px-3 py-2 text-sm shadow-xl">
      <span className="text-error">●</span>
      <span className="flex-1 text-ink">{lastError}</span>
      <button
        aria-label="dismiss"
        onClick={() => setError(null)}
        className="touch-target flex h-7 w-7 items-center justify-center rounded text-muted hover:text-ink"
      >
        <X size={14} />
      </button>
    </div>
  )
}