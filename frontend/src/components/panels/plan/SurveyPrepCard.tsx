// U9: "prepare survey area" card (H9). It reads the cached HRDEM session for
// the current AOI (centre + size) through the backend `survey_prepare_status`
// command — which runs `dem-prep hrdem-status`, pure filesystem — and shows
// the metadata the operator needs before flying. The Prepare button (U9b)
// fetches the DTM+DSM with streamed progress and a Cancel action.
//
// Failure/next-step text comes from the backend (`status.message` for the
// resting state, the command error for a failed prepare); the headline is
// classified locally so the operator sees a plain-language reason.

import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { invoke } from '@tauri-apps/api/core'
import type { GeoPoint } from '../../../generated-types/GeoPoint'
import type { SurveyPrepStatus } from '../../../generated-types/SurveyPrepStatus'
import type { SurveyPrepState } from '../../../generated-types/SurveyPrepState'
import { formatBytes } from '../../../util/units'
import { useTerrainStore } from '../../../stores/terrain'
import { classifyPrepError } from '../../../util/surveyPrep'

/** Coalesce the frequent centre/size edits into one status query. */
const REFRESH_DEBOUNCE_MS = 400

/** State chip tone (text comes from `survey.prep.state.*`). */
const STATE_TONE: Record<SurveyPrepState, string> = {
  ready: 'text-ok',
  partial: 'text-warn',
  absent: 'text-muted',
}

function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}


export default function SurveyPrepCard({
  center,
  size,
}: {
  center: GeoPoint | null
  size: { width: number; height: number }
}) {
  const { t } = useTranslation()
  const [status, setStatus] = useState<SurveyPrepStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [preparing, setPreparing] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const prepLine = useTerrainStore((s) => s.surveyPrep?.line ?? '')
  const setCoverage = useTerrainStore((s) => s.setCoverage)

  const lat = center?.latitude_deg
  const lon = center?.longitude_deg
  const { width, height } = size

  const refresh = useCallback(async () => {
    if (!inTauri() || lat === undefined || lon === undefined) return
    setLoading(true)
    try {
      const s = await invoke<SurveyPrepStatus>('survey_prepare_status', {
        lat,
        lon,
        widthM: width,
        heightM: height,
        outDir: null,
      })
      setStatus(s)
      setError(null)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [lat, lon, width, height])

  useEffect(() => {
    if (!inTauri() || lat === undefined || lon === undefined) {
      setStatus(null)
      setError(null)
      return
    }
    const timer = setTimeout(() => {
      void refresh()
    }, REFRESH_DEBOUNCE_MS)
    return () => clearTimeout(timer)
  }, [lat, lon, width, height, refresh])

  // Publish the prepared footprint to the map; clear when this card (the only
  // source of the bounds) leaves the planning view.
  useEffect(() => {
    setCoverage(status?.bounds ?? null)
    return () => setCoverage(null)
  }, [status?.bounds, setCoverage])

  const onPrepare = async () => {
    if (lat === undefined || lon === undefined) return
    setActionError(null)
    setPreparing(true)
    try {
      const result = await useTerrainStore.getState().prepareSurvey(lat, lon, width, height)
      if (result) setStatus(result)
      else setStatus(null) // cancelled: leave the resting state to the effect
    } catch (e) {
      setActionError(String(e))
    } finally {
      setPreparing(false)
    }
  }

  const onCancel = () => {
    void useTerrainStore.getState().cancelSurvey()
  }

  if (!inTauri()) return null

  const failKind = actionError ? classifyPrepError(actionError) : null

  return (
    <div className="rounded border border-line bg-canvas p-2 text-xs">
      <div className="flex items-center justify-between">
        <span className="font-medium text-ink">{t('survey.prep.title')}</span>
        {status && (
          <span className={`text-[11px] ${STATE_TONE[status.state]}`}>
            {t(`survey.prep.state.${status.state}`)}
          </span>
        )}
      </div>

      {lat === undefined || lon === undefined ? (
        <div className="mt-1 text-[11px] text-muted">{t('survey.prep.noCentre')}</div>
      ) : error ? (
        <div className="mt-1 text-[11px] text-error">{error}</div>
      ) : !status ? (
        <div className="mt-1 text-[11px] text-muted">
          {loading ? t('survey.prep.loading') : t('survey.prep.hint')}
        </div>
      ) : (
        <div className="mt-1 space-y-0.5">
          {status.state === 'ready' && (
            <div className="text-[11px] text-ok">{t('survey.prep.offlineReady')}</div>
          )}
          {status.project_name && (
            <Row label={t('survey.prep.project')} value={status.project_name} />
          )}
          {status.acquisition && (
            <Row label={t('survey.prep.acquisition')} value={status.acquisition} />
          )}
          {status.resolution_m != null && (
            <Row
              label={t('survey.prep.resolution')}
              value={t('survey.prep.resolutionValue', { m: status.resolution_m })}
            />
          )}
          <Row
            label={t('survey.prep.rasters')}
            value={[
              status.has_dtm ? 'DTM' : null,
              status.has_dsm ? 'DSM' : null,
            ]
              .filter(Boolean)
              .join(' · ') || t('survey.prep.none')}
          />
          <Row
            label={t('survey.prep.datum')}
            value={
              status.datum_verified
                ? t('survey.prep.datumVerified')
                : t('survey.prep.datumUnverified')
            }
            tone={status.datum_verified ? 'text-ok' : 'text-warn'}
          />
          {status.bytes != null && (
            <Row label={t('survey.prep.size')} value={formatBytes(status.bytes)} />
          )}
          {status.folder && (
            <div className="truncate text-[10px] text-muted" title={status.folder}>
              {status.folder}
            </div>
          )}
          {status.message && (
            <div className="pt-0.5 text-[11px] text-muted">{status.message}</div>
          )}
        </div>
      )}

      {failKind && (
        <div className="mt-1 rounded bg-error/10 p-1 text-[11px]">
          <div className="text-error">{t(`survey.prep.fail.${failKind}.title`)}</div>
          <div className="text-muted">{t(`survey.prep.fail.${failKind}.next`)}</div>
          <div className="truncate text-[10px] text-muted" title={actionError ?? ''}>
            {actionError}
          </div>
        </div>
      )}

      {lat !== undefined && lon !== undefined && (
        <div className="mt-1.5">
          {preparing ? (
            <div className="flex items-center gap-2">
              <span className="min-w-0 flex-1 truncate text-[11px] text-muted" title={prepLine}>
                {prepLine || t('survey.prep.preparing')}
              </span>
              <button
                type="button"
                onClick={onCancel}
                className="shrink-0 rounded border border-line px-2 py-0.5 text-[11px] hover:bg-elevated"
              >
                {t('survey.prep.cancel')}
              </button>
            </div>
          ) : (
            <button
              type="button"
              onClick={() => void onPrepare()}
              className="w-full rounded border border-line px-2 py-1 text-[11px] hover:bg-elevated"
            >
              {t('survey.prep.prepare')}
            </button>
          )}
        </div>
      )}
    </div>
  )
}

function Row({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="text-muted">{label}</span>
      <span className={`mono truncate text-right ${tone ?? 'text-ink'}`} title={value}>
        {value}
      </span>
    </div>
  )
}
