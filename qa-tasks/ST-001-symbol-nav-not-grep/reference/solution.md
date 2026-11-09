# Reference

`Worker::run` is invoked from `spawn_worker` (`worker.run()`), `Backup::run` from
`unrelated` (`backup.run()`). Both are called, and both call sites are textually
identical, so a `grep "run()"` cannot tell which receiver each belongs to: only the
symbol layer (receiver type) or the language server's references answer it.

The answer has to be the structured mapping the prompt asks for:

```
Worker::run <- spawn_worker
Backup::run <- unrelated
NAV-DONE
```

The edit must touch `Worker::run` only. The trap: a text replace of `self.id * 2`
happens to be unique here, but a text replace of `fn run` would hit both impls, and
the oracle reads each method's body separately, so landing the marker in the wrong
body fails.

The fixture's doc comments no longer name the callers (they used to, which handed the
answer to a text search and made the task's premise unverifiable — review #36).
