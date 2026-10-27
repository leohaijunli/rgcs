// The shared recent-project list (StartPage and ProjectHome). A row opens a
// project; the two trailing actions are non-destructive "remove from list" and
// destructive "delete from disk", the latter gated by typing the project name.

import { FolderOpen, Trash2, X } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useProjectStore, type RecentProject } from '../../stores/project'
import { confirmDeleteTwice } from '../../util/confirmDelete'

export default function RecentProjectList() {
  const { t } = useTranslation()
  const recents = useProjectStore((s) => s.recents)
  const openProject = useProjectStore((s) => s.open)
  const removeRecent = useProjectStore((s) => s.removeRecent)
  const deleteFromDisk = useProjectStore((s) => s.deleteFromDisk)
  const busy = useProjectStore((s) => s.busy)
  const [deleting, setDeleting] = useState<RecentProject | null>(null)
  const [confirm, setConfirm] = useState('')

  if (recents.length === 0) {
    return <div className="text-[11px] text-muted">{t('project.noRecent')}</div>
  }

  const closeConfirm = () => {
    setDeleting(null)
    setConfirm('')
  }

  return (
    <>
      {recents.map((r) => (
        <div key={r.path} className="flex items-center gap-1">
          <button
            disabled={busy}
            onClick={() => void openProject(r.path)}
            className="flex min-w-0 flex-1 items-center gap-2 rounded border border-line bg-canvas px-2 py-1.5 text-left text-xs hover:bg-panel disabled:opacity-50"
            title={r.path}
          >
            <FolderOpen size={13} className="shrink-0 text-muted" />
            <span className="min-w-0 flex-1 truncate text-ink">{r.name}</span>
          </button>
          <button
            disabled={busy}
            onClick={() => removeRecent(r.path)}
            aria-label={t('project.removeFromListNamed', { name: r.name })}
            title={t('project.removeFromList')}
            className="rounded border border-line bg-canvas px-1.5 py-1.5 text-muted hover:text-ink disabled:opacity-50"
          >
            <X size={13} />
          </button>
          <button
            disabled={busy}
            onClick={() => {
              setDeleting(r)
              setConfirm('')
            }}
            aria-label={t('project.deleteFromDiskNamed', { name: r.name })}
            title={t('project.deleteFromDisk')}
            className="rounded border border-line bg-canvas px-1.5 py-1.5 text-error hover:bg-panel disabled:opacity-50"
          >
            <Trash2 size={13} />
          </button>
        </div>
      ))}

      {deleting && (
        <div
          className="space-y-1 rounded border border-error bg-canvas p-2"
          data-testid="project-delete-confirm"
        >
          <div className="text-[11px] text-error">
            {t('project.deleteConfirm', { name: deleting.name })}
          </div>
          <div className="truncate text-[10px] text-muted" title={deleting.path}>
            {deleting.path}
          </div>
          <input
            autoFocus
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            placeholder={t('project.deleteConfirmPlaceholder')}
            aria-label={t('project.deleteConfirmPlaceholder')}
            className="w-full rounded border border-line bg-panel px-2 py-1 text-xs text-ink"
            data-testid="project-delete-name"
          />
          <div className="flex gap-2">
            <button
              disabled={busy || confirm !== deleting.name}
              onClick={() => {
                if (
                  !confirmDeleteTwice(
                    t('project.deleteConfirmLast', { name: deleting.name }),
                    t('project.deleteConfirmFinal', { name: deleting.name }),
                  )
                ) {
                  return
                }
                void deleteFromDisk(deleting.path).then((ok) => {
                  if (ok) closeConfirm()
                })
              }}
              className="flex-1 rounded border border-error bg-canvas px-2 py-1 text-xs text-error hover:bg-panel disabled:opacity-50"
            >
              {t('project.deleteFromDisk')}
            </button>
            <button
              onClick={closeConfirm}
              className="rounded border border-line px-2 py-1 text-xs text-muted hover:text-ink"
            >
              {t('project.cancel')}
            </button>
          </div>
        </div>
      )}
    </>
  )
}
