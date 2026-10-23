// The shared new-project form (StartPage and ProjectHome). Deliberately
// minimal: the operator supplies only a name. CRS and vertical datum use the
// project defaults, and the project is created under the default projects
// folder unless the operator picks another location.

import { Plus } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { open as pickFolder } from '@tauri-apps/plugin-dialog'
import { isTauri } from '../../inspector/mock'
import { defaultProjectsDir, slugifyProjectName, useProjectStore } from '../../stores/project'

interface ProjectWizardProps {
  /** Called after the project was created and opened. */
  onCreated: () => void
  onCancel: () => void
}

export default function ProjectWizard({ onCreated, onCancel }: ProjectWizardProps) {
  const { t } = useTranslation()
  const create = useProjectStore((s) => s.create)
  const busy = useProjectStore((s) => s.busy)
  const [name, setName] = useState('')
  const [dir, setDir] = useState<string | null>(null)
  const [defaultRoot, setDefaultRoot] = useState<string | null>(null)

  useEffect(() => {
    if (!isTauri()) return
    let cancelled = false
    void defaultProjectsDir()
      .then((root) => {
        if (!cancelled) setDefaultRoot(root)
      })
      .catch(() => {
        if (!cancelled) setDefaultRoot(null)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const pickDir = async () => {
    if (!isTauri()) return
    const picked = await pickFolder({ multiple: false, directory: true })
    if (typeof picked === 'string') setDir(picked)
  }

  const location = dir ?? (defaultRoot ? `${defaultRoot}/${slugifyProjectName(name)}` : null)

  const submit = async () => {
    if (name.trim().length === 0 || busy) return
    if (await create(name.trim(), dir ?? undefined)) onCreated()
  }

  return (
    <div className="space-y-2 rounded border border-line bg-canvas p-2" data-testid="project-wizard">
      <input
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void submit()
        }}
        placeholder={t('project.namePlaceholder')}
        aria-label={t('project.name')}
        className="w-full rounded border border-line bg-panel px-2 py-1 text-xs text-ink"
        data-testid="project-name"
      />
      <div className="flex items-center justify-between gap-2 text-[11px] text-muted">
        <span
          className="min-w-0 truncate"
          title={location ?? undefined}
          data-testid="project-location"
        >
          {location ?? t('project.defaultLocation')}
        </span>
        <button
          type="button"
          onClick={() => void pickDir()}
          disabled={busy}
          className="shrink-0 text-ink hover:underline disabled:opacity-50"
        >
          {t('project.changeLocation')}
        </button>
      </div>
      {dir && (
        <button
          type="button"
          onClick={() => setDir(null)}
          className="text-[11px] text-muted hover:text-ink"
        >
          {t('project.useDefaultLocation')}
        </button>
      )}
      <div className="flex gap-2">
        <button
          type="button"
          disabled={busy || name.trim().length === 0}
          onClick={() => void submit()}
          className="flex flex-1 items-center justify-center gap-1 rounded border border-ok bg-canvas px-2 py-1 text-xs text-ok hover:bg-panel disabled:opacity-50"
        >
          <Plus size={13} />
          {t('project.create')}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="rounded border border-line px-2 py-1 text-xs text-muted hover:text-ink"
        >
          {t('project.cancel')}
        </button>
      </div>
    </div>
  )
}
