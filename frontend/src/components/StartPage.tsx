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
import AppLogo from './AppLogo'
import CompanyLogo from './CompanyLogo'
import ProjectWizard from './project/ProjectWizard'
import RecentProjectList from './project/RecentProjectList'

export default function StartPage() {
  const { t } = useTranslation()
  const openProject = useProjectStore((s) => s.open)
  const error = useProjectStore((s) => s.error)
  const busy = useProjectStore((s) => s.busy)
  const continueWithoutProject = useProjectStore((s) => s.continueWithoutProject)
  const [creating, setCreating] = useState(false)

  const pickAnd = async (run: (dir: string) => Promise<boolean>) => {
    if (!isTauri()) return
    const dir = await pickFolder({ multiple: false, directory: true })
    if (typeof dir === 'string') await run(dir)
  }

  const armed = error === 'armed-vehicle'

  return (
    <div className="flex h-screen flex-col items-center overflow-y-auto bg-canvas px-4 py-6" data-testid="start-page">
      <div className="my-auto w-full max-w-[360px] shrink-0">
        <div className="mb-4 text-center">
          <AppLogo className="start-app-logo" />
          <div className="mt-1 text-xs text-muted">{t('start.subtitle')}</div>
        </div>

        {armed && <div className="mb-2 text-[11px] text-error">{t('project.armedRefusal')}</div>}
        {!armed && error && <div className="mb-2 text-[11px] text-error">{error}</div>}

        <div className="space-y-2 rounded border border-line bg-panel p-3">
          <div className="text-xs font-medium text-ink">{t('start.recent')}</div>
          <div className="space-y-1">
            <RecentProjectList />
          </div>

          <button
            disabled={busy}
            onClick={() => void pickAnd((dir) => openProject(dir))}
            className="w-full rounded border border-line bg-canvas px-2 py-1.5 text-xs text-ink hover:bg-panel disabled:opacity-50"
          >
            {t('project.open')}
          </button>

          {creating ? (
            <ProjectWizard
              onCreated={() => setCreating(false)}
              onCancel={() => setCreating(false)}
            />
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
        <div className="mt-5 text-center">
          <CompanyLogo className="start-company-logo" />
        </div>
      </div>
    </div>
  )
}
