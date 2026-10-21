/** Whether the event target is a text-entry element (typing shortcuts must
 * not fire). Shared by the undo/redo hook and the `?` shortcut sheet. */
export function isTextEntry(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  return (
    el != null &&
    (el.tagName === 'INPUT' ||
      el.tagName === 'TEXTAREA' ||
      el.tagName === 'SELECT' ||
      el.isContentEditable)
  )
}
