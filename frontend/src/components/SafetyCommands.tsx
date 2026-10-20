// Always-visible safety commands (plan §4 L2): RTL and Hold live on the nav
// bar in every view instead of only inside the Flight dropdown, so the two
// actions an operator reaches for under stress are never behind a menu.
//
// They are the only command buttons that ignore an in-flight command. A7 made
// the backend the arbiter — an emergency command preempts a normal one, and
// the outcome comes back as `superseded`/`busy` on the status strip — so a
// caller that tried to decide the priority itself would be a second copy of
// that rule. The dropdown keeps its own `pending` gate.
//
// Risk grading follows U4: RTL moves the aircraft, so it is hold-to-send (the
// confirm bar rises while the ring fills, showing the mode, height above
// ground and battery the command would run against); Hold is a benign state
// change, so it is one click and lights up when the vehicle is already there.

import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Button, HoldButton } from '../design-system/components'
import { useCommandStore } from '../stores/command'
import { useLinkStore } from '../stores/link'
import { useTelemetryStore } from '../stores/telemetry'
import { AUTO_SUB_LOITER, HOLD_CUSTOM_MODE, MAIN_MODE_AUTO } from '../util/px4mode'
import ConfirmContextBar from './ConfirmContextBar'

export default function SafetyCommands() {
  const { t } = useTranslation()
  const link = useLinkStore((s) => s.link)
  const heartbeat = useTelemetryStore((s) => s.snapshot?.heartbeat ?? null)
  const send = useCommandStore((s) => s.send)
  const sendSetMode = useCommandStore((s) => s.sendSetMode)
  const [reading, setReading] = useState(false)

  const linkUp = link?.fc_alive === true
  const armed = heartbeat?.base_mode.safety_armed === true
  const inHold = heartbeat?.custom_mode === HOLD_CUSTOM_MODE

  // PX4 only returns home once it has armed (the mode table's `return` rule:
  // armed + home valid); the button says so instead of failing on the ack.
  const rtlDisabled = !linkUp || !armed
  const rtlTitle = !linkUp
    ? t('plan.cmd.noLink')
    : !armed
      ? t('plan.cmd.rtlNotArmed')
      : t('plan.cmd.willRtl')

  return (
    <div className="relative flex items-center gap-1">
      <HoldButton
        data-testid="rtl-always"
        size="md"
        variant="danger"
        title={rtlTitle}
        hint={t('plan.cmd.willRtl')}
        disabled={rtlDisabled}
        onHoldChange={setReading}
        onConfirm={() => void send('rtl')}
      >
        {t('plan.rtl')}
      </HoldButton>
      <Button
        data-testid="hold-always"
        data-mode-current={inHold}
        size="md"
        variant={inHold ? 'primary' : 'default'}
        disabled={!linkUp}
        title={linkUp ? t('plan.cmd.willHold') : t('plan.cmd.noLink')}
        onClick={() => void sendSetMode(MAIN_MODE_AUTO, AUTO_SUB_LOITER)}
      >
        {t('plan.cmd.holdMode')}
      </Button>
      {reading && (
        <div className="absolute right-0 top-full z-50 mt-1">
          <ConfirmContextBar />
        </div>
      )}
    </div>
  )
}
