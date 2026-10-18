# Reference

`Worker::run` is invoked from `spawn_worker` (line: `worker.run()`), `Backup::run`
from `unrelated` (line: `backup.run()`). Both are textually identical calls, so a
`grep "run()"` cannot tell which receiver each belongs to: only the symbol layer
(receiver type) or the language server's references answer it.

The edit must touch `Worker::run` only. The trap: a text replace of
`self.id * 2` happens to be unique here, but a text replace of `fn run` would hit
both impls.
