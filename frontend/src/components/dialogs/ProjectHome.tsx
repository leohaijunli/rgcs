// Project home (PJ3): the start surface — recent projects, create and open —
// plus the open project's survey list and session history. Reached from the
// TopBar's project pill; also the U14 first-run surface (no project open →
// this is what the pill opens onto).
//
// Switching or closing while the vehicle is armed is refused by the store
// (`armed-vehicle`), and the reason is stated here (A1 policy; the
// surveying-state refinement lands with M1 ⏸).

import * as Dialog from '@radix-ui/react-dialog'
import { FileDown, FolderOpen, FolderUp, Plus, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { open as pickFolder, save as pickSave } from '@tauri-apps/plugin-dialog'
import { isTauri } from '../../inspector/mock'
import { useProjectStore } from '../../stores/project'
import { StatusDot } from '../../design-system/components'
import type { Session } from '../../generated-types/Session'
import type { Survey } from '../../generated-types/Survey'

const DEFAULT_CRS = 'EPSG:4326'
const DEFAULT_DATUM = 'cgvd2013'

/** Epoch microseconds → a short local date (the session list's date column).
 * The IPC delivers a JSON number; accept a bigint too for safety. */
function dateOf(us: number | bigint | null): string {
  if (us == null) return '—'
  const ms = typeof us === 'bigint' ? Number(us / 1000n) : Math.floor(us / 1000)
  return new Date(ms).toLocaleString()
}

function SurveyRow({ survey }: { survey: Survey }) {
  const { t } = useTranslation()
  const listSessions = useProjectStore((s) => s.listSessions)
  const [sessions, setSessions] = useState<Session[] | null>(null)
  const [open, setOpen] = useState(false)

  const toggle = () => {
    setOpen((o) => !o)
    if (sessions === null) void listSessions(survey.id).then(setSessions)
  }

  return (
    <div className="rounded border border-line bg-canvas text-xs">
      <button onClick={toggle} className="flex w-full items-center gap-2 px-2 py-1.5 text-left" data-testid="survey-row">
        <StatusDot
          tone={survey.status === 'complete' ? 'ok' : survey.status === 'flying' ? 'warn' : 'muted'}
        />
        <span className="min-w-0 flex-1 truncate text-ink" title={survey.name}>
          {survey.name}
        </span>
        <span className="mono rounded border border-line px-1 text-[10px] text-muted" title={t('project.kind')}>
          {survey.kind}
        </span>
        <span className="text-muted">{dateOf(survey.created_utc_us)}</span>
      </button>
      {open && (
        <div className="space-y-1 border-t border-line px-2 py-1.5" data-testid="session-list">
          {(sessions ?? []).map((s) => (
            <div key={s.id} className="flex items-center justify-between gap-2">
              <span className="mono truncate text-muted" title={s.id}>
                {s.id}
              </span>
              <span className="text-muted">{s.status}</span>
              <span className="text-muted">{dateOf(s.started_utc_us)}</span>
            </div>
          ))}
          {sessions !== null && sessions.length === 0 && (
            <div className="text-muted">{t('project.noSessions')}</div>
          )}
        </div>
      )}
    </div>
  )
}

export default function ProjectHome({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const { t } = useTranslation()
  const project = useProjectStore((s) => s.project)
  const surveys = useProjectStore((s) => s.surveys)
  const error = useProjectStore((s) => s.error)
  const busy = useProjectStore((s) => s.busy)
  const openProject = useProjectStore((s) => s.open)
  const createProject = useProjectStore((s) => s.create)
  const closeProject = useProjectStore((s) => s.close)
  const recent = useProjectStore((s) => s.recent)
  const [creating, setCreating] = useState(false)
  const [name, setName] = useState('')
  const [crs, setCrs] = useState(DEFAULT_CRS)
  const [datum, setDatum] = useState(DEFAULT_DATUM)

  const pickAnd = async (run: (dir: string) => Promise<boolean>) => {
    if (!isTauri()) return
    const dir = await pickFolder({ multiple: false, directory: true })
    if (typeof dir === 'string') await run(dir)
  }

  /** PJ4: export the open project to a zip via a save dialog. */
  const [exportNote, setExportNote] = useState<string | null>(null)
  const exportProject = async () => {
    if (!isTauri() || !project) return
    setExportNote(null)
    const dest = await pickSave({
      defaultPath: `${project.name.replace(/[^\w-]+/g, '_')}.zip`,
      filters: [{ name: 'Project archive', extensions: ['zip'] }],
    })
    if (typeof dest !== 'string') return
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const manifest = await invoke<{ project_id: string; files: string[] }>('project_export', {
        zipPath: dest,
      })
      setExportNote(`${manifest.files.length} files exported`)
    } catch (e) {
      setExportNote(String(e))
    }
  }

  /** PJ4: import a project zip next to the existing projects (the recent
   * list's parent, so the imported project shows up beside them). */
  const importProject = async () => {
    if (!isTauri()) return
    setExportNote(null)
    const { invoke } = await import('@tauri-apps/api/core')
    const zip = await pickFile()
    if (typeof zip !== 'string') return
    const first = recent()[0]?.path
    const projectsRoot = first ? first.split(/[\\/]/).slice(0, -1).join('/') : null
    if (!projectsRoot) {
      setExportNote('Import needs a known project folder first — open one project, then import.')
      return
    }
    try {
      const target = await invoke<string>('project_import', { zipPath: zip, projectsRoot })
      await openProject(target)
      setExportNote(null)
    } catch (e) {
      setExportNote(String(e))
    }
  }
  const pickFile = async (): Promise<string | null> => {
    const picked = await pickFolder({
      multiple: false,
      directory: false,
      filters: [{ name: 'Project archive', extensions: ['zip'] }],
    })
    return typeof picked === 'string' ? picked : null
  }

  useEffect(() => {
    if (!open) setCreating(false)
  }, [open])

  const armed = error === 'armed-vehicle'

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/50" />
        <Dialog.Content
          data-testid="project-home"
          className="fixed left-1/2 top-16 z-50 w-[340px] -translate-x-1/2 rounded border border-line bg-panel p-3 shadow-2xl"
        >
          <div className="mb-2 flex items-center justify-between">
            <Dialog.Title className="text-sm font-medium">{t('project.title')}</Dialog.Title>
            <Dialog.Close asChild>
              <button aria-label={t('project.close')} className="text-muted hover:text-ink">
                <X size={14} />
              </button>
            </Dialog.Close>
          </div>

          {exportNote && <div className="mb-2 text-[11px] text-muted">{exportNote}</div>}
          {armed && <div className="mb-2 text-[11px] text-error">{t('project.armedRefusal')}</div>}
          {!armed && error && <div className="mb-2 text-[11px] text-error">{error}</div>}

          {project === null ? (
            <div className="space-y-3" data-testid="project-picker">
              <div className="space-y-1">
                {recent().map((r) => (
                  <button
                    key={r.path}
                    disabled={busy}
                    onClick={() => void openProject(r.path)}
                    className="flex w-full items-center gap-2 rounded border border-line bg-canvas px-2 py-1.5 text-left text-xs hover:bg-panel disabled:opacity-50"
                    title={r.path}
                  >
                    <FolderOpen size={13} className="shrink-0 text-muted" />
                    <span className="min-w-0 flex-1 truncate text-ink">{r.name}</span>
                  </button>
                ))}
                {recent().length === 0 && <div className="text-[11px] text-muted">{t('project.noRecent')}</div>}
              </div>

              <button
                disabled={busy}
                onClick={() => void pickAnd((dir) => openProject(dir))}
                className="flex w-full items-center gap-2 rounded border border-line bg-canvas px-2 py-1.5 text-xs text-ink hover:bg-panel disabled:opacity-50"
              >
                <FolderOpen size={13} />
                {t('project.open')}
              </button>

              <button
                disabled={busy}
                data-testid="project-import"
                onClick={() => void importProject()}
                className="flex w-full items-center gap-2 rounded border border-line bg-canvas px-2 py-1.5 text-xs text-ink hover:bg-panel disabled:opacity-50"
              >
                <FolderUp size={13} />
                {t('project.import')}
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
                    className="w-full rounded border border-line bg-panel px-2 py-1 text-xs text-ink"
                    aria-label={t('project.crs')}
                  />
                  <select
                    value={datum}
                    onChange={(e) => setDatum(e.target.value)}
                    className="w-full rounded border border-line bg-panel px-2 py-1 text-xs text-ink"
                    aria-label={t('project.datum')}
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
                    <button onClick={() => setCreating(false)} className="rounded border border-line px-2 py-1 text-xs text-muted hover:text-ink">
                      {t('project.cancel')}
                    </button>
                  </div>
                </div>
              ) : (
                <button
                  disabled={busy}
                  onClick={() => setCreating(true)}
                  className="flex w-full items-center gap-2 rounded border border-line bg-canvas px-2 py-1.5 text-xs text-ink hover:bg-panel disabled:opacity-50"
                >
                  <Plus size={13} />
                  {t('project.new')}
                </button>
              )}
            </div>
          ) : (
            <div className="space-y-3" data-testid="project-open-view">
              <div className="rounded border border-line bg-canvas px-2 py-1.5 text-xs">
                <div className="flex items-center justify-between">
                  <span className="font-medium text-ink">{project.name}</span>
                  <span className="mono text-muted">{project.crs}</span>
                </div>
                <div className="mt-0.5 flex items-center justify-between text-muted">
                  <span>{project.vertical_datum}</span>
                  <span>{dateOf(project.created_utc_us)}</span>
                </div>
              </div>

              <div className="space-y-1">
                {surveys.map((s) => (
                  <SurveyRow key={s.id} survey={s} />
                ))}
                {surveys.length === 0 && <div className="text-[11px] text-muted">{t('project.noSurveys')}</div>}
              </div>

              <button
                disabled={busy}
                data-testid="project-export"
                onClick={() => void exportProject()}
                className="flex w-full items-center gap-2 rounded border border-line bg-canvas px-2 py-1.5 text-xs text-ink hover:bg-panel disabled:opacity-50"
              >
                <FileDown size={13} />
                {t('project.export')}
              </button>

              <button
                disabled={busy}
                onClick={() => void closeProject()}
                className="w-full rounded border border-line bg-canvas px-2 py-1.5 text-xs text-muted hover:bg-panel hover:text-ink disabled:opacity-50"
              >
                {t('project.closeProject')}
              </button>
            </div>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
