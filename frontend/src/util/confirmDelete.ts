/**
 * Every destructive UI action asks twice. The two prompts are deliberately
 * separate so a stray Enter cannot run both; declining either cancels.
 */
export function confirmDeleteTwice(first: string, last: string): boolean {
  if (!window.confirm(first)) return false
  return window.confirm(last)
}
