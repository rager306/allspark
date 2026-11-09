- Hardened the daemon socket trust boundary: the blocking CLI client and the
  server-side `accept` now refuse a socket owned by another local user, a
  supervisor-shutdown `--force` no longer signals a pid a forged greeting names,
  and a direct session ticket is refused for a client-owned session.
- Cron durability: an unreadable jobs file is refused instead of being
  overwritten, a failed session-file read no longer cancels a live schedule, an
  interrupted one-shot is re-armed instead of completed, and a fire that outlives
  its settle window while running is no longer recorded as skipped.
