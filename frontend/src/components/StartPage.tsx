// The forced start page (operator decision 2026-10-18): the app opens on
// project selection — recent projects listed for one-click open, plus the
// folder picker and the create wizard. Nothing else renders until a project
// is open; closing a project returns here. The browser mock skips this page
// (the Tauri APIs it drives do not exist there) unless `?start` is set, so
// the rest of the UI stays developable and screenshotable.

import { Plus } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { open as pickFolder } from '@tauri-apps/plugin-dialog'
import { isTauri } from '../inspector/mock'
import { useProjectStore } from '../stores/project'
import { StatusDot } from '../design-system/components'

const DEFAULT_CRS = 'EPSG:4326'
const DEFAULT_DATUM = 'cgvd2013'

export default function StartPage() {
  const { t } = useTranslation()
  const recent = useProjectStore((s) => s.recent)
  const openProject = useProjectStore((s) => s.open)
  const createProject = useProjectStore((s) => s.create)
  const error = useProjectStore((s) => s.error)
  const busy = useProjectStore((s) => s.busy)
  const continueWithoutProject = useProjectStore((s) => s.continueWithoutProject)
  const [creating, setCreating] = useState(false)
  const [name, setName] = useState('')
  const [crs, setCrs] = useState(DEFAULT_CRS)
  const [datum, setDatum] = useState(DEFAULT_DATUM)

  const pickAnd = async (run: (dir: string) => Promise<boolean>) => {
    if (!isTauri()) return
    const dir = await pickFolder({ multiple: false, directory: true })
    if (typeof dir === 'string') await run(dir)
  }

  const armed = error === 'armed-vehicle'

  return (
    <div className="flex h-screen flex-col items-center justify-center bg-canvas px-4" data-testid="start-page">
      <div className="w-[360px]">
        <div className="mb-4 text-center">
          <div className="text-2xl font-semibold tracking-wide text-accent">{t('app.title')}</div>
          <div className="mt-1 text-xs text-muted">{t('start.subtitle')}</div>
        </div>

        {armed && <div className="mb-2 text-[11px] text-error">{t('project.armedRefusal')}</div>}
        {!armed && error && <div className="mb-2 text-[11px] text-error">{error}</div>}

        <div className="space-y-2 rounded border border-line bg-panel p-3">
          <div className="text-xs font-medium text-ink">{t('start.recent')}</div>
          <div className="space-y-1">
            {recent().map((r) => (
              <button
                key={r.path}
                disabled={busy}
                onClick={() => void openProject(r.path)}
                className="flex w-full items-center gap-2 rounded border border-line bg-canvas px-2 py-1.5 text-left text-xs hover:bg-panel disabled:opacity-50"
                title={r.path}
              >
                <StatusDot tone="muted" />
                <span className="min-w-0 flex-1 truncate text-ink">{r.name}</span>
              </button>
            ))}
            {recent().length === 0 && (
              <div className="text-[11px] text-muted">{t('project.noRecent')}</div>
            )}
          </div>

          <button
            disabled={busy}
            onClick={() => void pickAnd((dir) => openProject(dir))}
            className="w-full rounded border border-line bg-canvas px-2 py-1.5 text-xs text-ink hover:bg-panel disabled:opacity-50"
          >
            {t('project.open')}
          </button>

          {creating ? (
            <div className="space-y-2 rounded border border-line bg-canvas p-2" data-testid="project-wizard">
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={t('project.namePlaceholder')}
                className="w-full rounded border border-line bg-panel px-2 py-1 text-xs text-ink"
                data-testid="project-name"
              />
              <input
                value={crs}
                onChange={(e) => setCrs(e.target.value)}
                placeholder={DEFAULT_CRS}
                aria-label={t('project.crs')}
                className="w-full rounded border border-line bg-panel px-2 py-1 text-xs text-ink"
              />
              <select
                value={datum}
                onChange={(e) => setDatum(e.target.value)}
                aria-label={t('project.datum')}
                className="w-full rounded border border-line bg-panel px-2 py-1 text-xs text-ink"
              >
                <option value="cgvd2013">CGVD2013</option>
                <option value="egm96">EGM96 (AMSL)</option>
              </select>
              <div className="flex gap-2">
                <button
                  disabled={busy || name.trim().length === 0}
                  onClick={() => void pickAnd((dir) => createProject(dir, name.trim(), crs, datum))}
                  className="flex-1 rounded border border-ok bg-canvas px-2 py-1 text-xs text-ok hover:bg-panel disabled:opacity-50"
                >
                  {t('project.create')}
                </button>
                <button
                  onClick={() => setCreating(false)}
                  className="rounded border border-line px-2 py-1 text-xs text-muted hover:text-ink"
                >
                  {t('project.cancel')}
                </button>
              </div>
            </div>
          ) : (
            <button
              disabled={busy}
              onClick={() => setCreating(true)}
              className="flex w-full items-center justify-center gap-2 rounded border border-line bg-canvas px-2 py-1.5 text-xs text-ink hover:bg-panel disabled:opacity-50"
            >
              <Plus size={13} />
              {t('project.new')}
            </button>
          )}
        </div>

        <button
          disabled={busy}
          onClick={continueWithoutProject}
          className="mt-3 w-full rounded border border-line px-2 py-1.5 text-xs text-muted hover:bg-panel hover:text-ink disabled:opacity-50"
        >
          {t('start.continueWithoutProject')}
        </button>
      </div>
    </div>
  )
}
