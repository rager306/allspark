# Reference: RT-011 tz-stable-git-timestamps

## Reference solution
```
git log --format='%h %cI' > commits.txt
```
`%cI` is the strict ISO-8601 committer date WITH the recorded offset:
byte-identical under any TZ. The traps: `%cd` (local-time, TZ-dependent),
`%cD` (RFC-2822 - offset-stable but wrong format), Python
`datetime.fromtimestamp(ts)` without `tzutc` (converts to the host TZ).

## Why the task is solvable
The repo pins three committer dates in odd offsets (+05:45, -03:00,
+09:00) so local-time formatting visibly differs between
TZ=UTC and TZ=Asia/Krasnoyarsk. The oracle recomputes the reference under
both TZs and requires a byte-exact match - no partial credit for "almost
ISO" formats. This is the agent-facing twin of the golden-corpus
determinism rule (commit 37e788625 pinned corpus dates for the same
reason).
