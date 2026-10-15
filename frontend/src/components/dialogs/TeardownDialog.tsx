// The one confirmation every in-app teardown goes through (plan §4 L4).
//
// Two things must be true before a disconnect or a quit is sent, and the
// shell answers both: the vehicle state must allow it (armed / airborne is
// refused outright — there is no keyword that talks past that), and, when
// local plan work would be lost, the operator must type the keyword the shell
// names. The window close button never comes through here: the shell forces
// that one and raises a warn alert about whatever it walked past.

import { useEffect, useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { AlertTriangle } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import type { TeardownGuard } from '../../generated-types/TeardownGuard'
import { runTeardown, teardownGuard } from '../../desktop/connect'
import { useMissionStore } from '../../stores/mission'
import { useTeardownStore } from '../../stores/teardown'

export default function TeardownDialog() {
  const { t } = useTranslation()
  const reason = useTeardownStore((s) => s.pending)
  const close = useTeardownStore((s) => s.close)
  const [guard, setGuard] = useState<TeardownGuard | null>(null)
  const [typed, setTyped] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Local work that exists only on this screen. The mission store owns the
  // "edited since the last sync" flag; the count is for the message.
  const atStake = useMissionStore((s) => (s.dirty ? s.waypoints.length + s.blocks.length : 0))

  useEffect(() => {
    if (reason === null) return
    let cancelled = false
    setGuard(null)
    setTyped('')
    setError(null)
    setBusy(false)
    void teardownGuard(reason, atStake > 0)
      .then((g) => {
        if (!cancelled) setGuard(g)
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      cancelled = true
    }
  }, [reason, atStake])

  const refusal = guard?.refusal ?? null
  const keyword = guard?.keyword ?? null
  const ready = guard !== null && refusal === null && (keyword === null || typed === keyword)

  const confirm = async () => {
    if (reason === null || !ready || busy) return
    setBusy(true)
    setError(null)
    try {
      await runTeardown(reason)
      close()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  const title = reason === 'disconnect' ? t('teardown.disconnect') : t('teardown.quit')

  return (
    <Dialog.Root open={reason !== null} onOpenChange={(open) => !open && close()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/50" />
        <Dialog.Content
          data-testid="teardown-dialog"
          className="fixed left-1/2 top-1/2 z-50 w-[min(94vw,460px)] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-line bg-panel p-4 shadow-2xl"
        >
          <Dialog.Title className="text-sm font-medium text-ink">{title}</Dialog.Title>
          <Dialog.Description className="sr-only">{title}</Dialog.Description>

          {refusal !== null && (
            <div className="mt-3 rounded border border-error px-3 py-2 text-xs">
              <div data-testid="teardown-refusal" className="flex items-center gap-2 text-error">
                <AlertTriangle size={14} className="shrink-0" />
                <span>{t('teardown.refused', { reason: refusal })}</span>
              </div>
              <div className="mt-1 text-muted">{t('teardown.refusedHint')}</div>
            </div>
          )}

          {guard !== null && refusal === null && (
            <div className="mt-3 space-y-3">
              {atStake > 0 && (
                <div data-testid="teardown-at-stake" className="text-xs text-warn">
                  {t('teardown.unsaved', { count: atStake })}
                </div>
              )}
              {keyword !== null && (
                <label className="block text-xs text-muted">
                  <span className="mb-1 block">{t('teardown.typeKeyword', { keyword })}</span>
                  <input
                    data-testid="teardown-keyword"
                    value={typed}
                    autoFocus
                    spellCheck={false}
                    autoComplete="off"
                    onChange={(e) => setTyped(e.target.value)}
                    placeholder={keyword}
                    className="mono h-9 w-full rounded border border-line bg-canvas px-2 text-sm text-ink outline-none focus:border-accent"
                  />
                </label>
              )}
            </div>
          )}

          {error !== null && (
            <div data-testid="teardown-error" className="mt-3 text-xs text-error">
              {t('teardown.failed', { message: error })}
            </div>
          )}

          <div className="mt-4 flex justify-end gap-2">
            <button
              data-testid="teardown-cancel"
              onClick={close}
              className="touch-target h-9 rounded border border-line bg-canvas px-3 text-xs text-ink hover:bg-panel"
            >
              {t('teardown.cancel')}
            </button>
            {refusal === null && (
              <button
                data-testid="teardown-confirm"
                onClick={() => void confirm()}
                disabled={!ready || busy}
                title={ready ? undefined : t('teardown.keywordRequired')}
                className="touch-target h-9 rounded border border-error bg-canvas px-3 text-xs font-medium text-error disabled:opacity-50"
              >
                {busy ? t('teardown.working') : t('teardown.confirm')}
              </button>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
