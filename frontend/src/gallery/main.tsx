// Component gallery (UI2): every design-system primitive plus the token
// palette on one page, so a theme change can be reviewed at a glance.
// Dev-only entry — not part of the packaged app.

import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Crosshair, Layers, Radio } from 'lucide-react'
import {
  Button,
  CommandStatusStrip,
  HoldButton,
  IconButton,
  Panel,
  Popover,
  PopoverAction,
  PopoverToggle,
  StatusDot,
  Tabs,
  TaskProgress,
  type Tone,
} from '../design-system/components'
import '../design-system/index.css'

/** Colour tokens shown as swatches (name comes from the CSS custom prop). */
const COLOR_TOKENS = [
  '--mg-bg',
  '--mg-bg-raised',
  '--mg-border',
  '--mg-ink',
  '--mg-muted',
  '--mg-accent',
  '--mg-ok',
  '--mg-warn',
  '--mg-error',
  '--mg-mag',
  '--mg-sky',
  '--mg-ground',
  '--mg-instrument',
  '--mg-trace-1',
  '--mg-trace-2',
  '--mg-trace-3',
  '--mg-trace-4',
  '--mg-trace-5',
  '--mg-trace-6',
  '--mg-trace-7',
  '--mg-trace-8',
]

const SPACING_TOKENS = ['--mg-space-1', '--mg-space-2', '--mg-space-3', '--mg-space-4', '--mg-space-6']
const TEXT_TOKENS = ['--mg-text-xs', '--mg-text-sm', '--mg-text-base', '--mg-text-lg']

const TONES: Tone[] = ['ok', 'warn', 'error', 'muted', 'accent']

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section style={{ marginBottom: 24 }}>
      <h2
        style={{
          fontSize: 13,
          textTransform: 'uppercase',
          letterSpacing: '0.06em',
          color: 'var(--mg-muted)',
          marginBottom: 8,
          borderBottom: '1px solid var(--mg-border)',
          paddingBottom: 4,
        }}
      >
        {title}
      </h2>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, alignItems: 'center' }}>
        {children}
      </div>
    </section>
  )
}

function Swatch({ name }: { name: string }) {
  return (
    <div style={{ width: 148 }}>
      <div
        style={{
          height: 44,
          borderRadius: 6,
          background: `var(${name})`,
          border: '1px solid var(--mg-border)',
        }}
      />
      <div className="mono" style={{ fontSize: 10, color: 'var(--mg-muted)', marginTop: 2 }}>
        {name}
      </div>
    </div>
  )
}

function Gallery() {
  const [theme, setTheme] = useState<'dark' | 'light'>(
    (document.documentElement.dataset.theme as 'dark' | 'light') || 'dark',
  )
  const [tab, setTab] = useState<'a' | 'b'>('a')
  const [on1, setOn1] = useState(true)
  const [on2, setOn2] = useState(false)

  const applyTheme = (t: 'dark' | 'light') => {
    document.documentElement.dataset.theme = t
    setTheme(t)
  }

  return (
    <div style={{ padding: 20, minHeight: '100%' }}>
      <header
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: 20,
        }}
      >
        <div>
          <div style={{ fontSize: 16, fontWeight: 600, color: 'var(--mg-accent)' }}>
            MagGCS design-system
          </div>
          <div style={{ fontSize: 11, color: 'var(--mg-muted)' }}>
            UI2 gallery — primitives + tokens, both themes
          </div>
        </div>
        <Tabs
          items={[
            { id: 'dark', label: 'Dark' },
            { id: 'light', label: 'Light' },
          ]}
          active={theme}
          onChange={(t) => applyTheme(t as 'dark' | 'light')}
        />
      </header>

      <Section title="Colour tokens">
        {COLOR_TOKENS.map((n) => (
          <Swatch key={n} name={n} />
        ))}
      </Section>

      <Section title="Spacing">
        {SPACING_TOKENS.map((n) => (
          <div key={n} style={{ width: 148 }}>
            <div style={{ height: 12, width: `var(${n})`, background: 'var(--mg-accent)' }} />
            <div className="mono" style={{ fontSize: 10, color: 'var(--mg-muted)' }}>
              {n}
            </div>
          </div>
        ))}
      </Section>

      <Section title="Typography">
        {TEXT_TOKENS.map((n) => (
          <div key={n} style={{ width: 200 }}>
            <div style={{ fontSize: `var(${n})`, color: 'var(--mg-ink)' }}>Aa 参数 123</div>
            <div className="mono" style={{ fontSize: 10, color: 'var(--mg-muted)' }}>
              {n}
            </div>
          </div>
        ))}
      </Section>

      <Section title="Button">
        <Button variant="default">Default</Button>
        <Button variant="primary">Primary</Button>
        <Button variant="ghost">Ghost</Button>
        <Button variant="danger">Danger</Button>
        <Button variant="default" size="sm">
          Small
        </Button>
        <Button variant="primary" disabled>
          Disabled
        </Button>
      </Section>

      <Section title="IconButton">
        <IconButton title="Crosshair">
          <Crosshair size={17} strokeWidth={1.75} />
        </IconButton>
        <IconButton title="Armed" active>
          <Radio size={17} strokeWidth={1.75} />
        </IconButton>
        <IconButton title="Layers" size={32}>
          <Layers size={15} strokeWidth={1.75} />
        </IconButton>
      </Section>

      <Section title="Tabs">
        <Tabs
          items={[
            { id: 'a', label: 'Option A' },
            { id: 'b', label: 'Option B' },
          ]}
          active={tab}
          onChange={setTab}
        />
      </Section>

      <Section title="HoldButton / ConfirmBar (U4)">
        <div className="flex items-center gap-3">
          <HoldButton hint="Takeoff" holdMs={1000} onConfirm={() => {}}>
            Takeoff
          </HoldButton>
          <span className="text-xs" style={{ color: 'var(--mg-muted)' }}>
            hold 1 s with a progress ring
          </span>
        </div>
      </Section>

      <Section title="TaskProgress (U13)">
        <div className="w-72 space-y-3">
          <TaskProgress label="Uploading mission" sent={12} total={30} startedAtMs={Date.now() - 4200} />
          <TaskProgress label="Building 3D tileset" indeterminate startedAtMs={Date.now() - 61_000} onCancel={() => {}} />
          <TaskProgress label="Failed stage" indeterminate danger startedAtMs={Date.now() - 900} />
        </div>
      </Section>

      <Section title="CommandStatusStrip (A7/U4)">
        <div className="w-96">
          <CommandStatusStrip command="RTL" phase="sent" elapsedMs={1200} tone="accent" />
        </div>
      </Section>

      <Section title="StatusDot">
        {TONES.map((tone) => (
          <StatusDot key={tone} tone={tone} title={tone} />
        ))}
      </Section>

      <Section title="Panel / Popover">
        <Panel className="w-64 p-3">
          <div style={{ fontSize: 12, color: 'var(--mg-ink)' }}>Panel surface</div>
          <div style={{ fontSize: 11, color: 'var(--mg-muted)' }}>bg-raised + border token</div>
        </Panel>
        <div style={{ position: 'relative', width: 220, height: 150, border: '1px dashed var(--mg-border)' }}>
          <Popover className="left-2 top-2 w-48">
            <PopoverToggle
              label="Hillshade"
              on={on1}
              onLabel="On"
              offLabel="Off"
              onToggle={() => setOn1(!on1)}
            />
            <PopoverAction label="Locate" icon={<Crosshair size={14} />} onClick={() => {}} />
            <PopoverToggle
              label="Grid"
              on={on2}
              onLabel="On"
              offLabel="Off"
              onToggle={() => setOn2(!on2)}
            />
          </Popover>
        </div>
      </Section>
    </div>
  )
}

createRoot(document.getElementById('root') as HTMLElement).render(<Gallery />)
