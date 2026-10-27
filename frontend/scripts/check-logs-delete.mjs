// Compatibility entry: deletion is now covered with the complete owned Shell/Logs
// browser fixture, including two confirmations and exact native log identity.
process.argv[2] ??= 'http://localhost:5199/'
process.argv[3] ??= '/tmp/opencode/logs-delete-acceptance'
await import('./check-shell-logs.mjs')
