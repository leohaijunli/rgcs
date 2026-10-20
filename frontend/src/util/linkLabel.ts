// FC status text shared by the top bar, vehicle drawer and settings dialog.
//
// `link_state` describes the transport (the socket) while `fc_alive` tracks
// PX4 heartbeat freshness. The two disagree on purpose: a UDP socket stays
// "connected" while the FC is silent, so the pill must not print "Connected"
// in red. Transport problems win, then heartbeat liveness.

import type { LinkStatus } from '../generated-types/LinkStatus'

type Translate = (key: string) => string

export function fcStatusLabel(t: Translate, link: LinkStatus | null): string {
  if (!link) return t('link.noLink')
  if (link.link_state !== 'connected') {
    return t(`link.${link.link_state.toLowerCase()}`)
  }
  return link.fc_alive ? t('link.connected') : t('link.lost')
}
