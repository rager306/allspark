# Reference

`main -> handle -> validate -> Repo::load` (and `main -> Repo::new`).

The chain crosses three files, so answering it by reading files means opening all
three and reconstructing the edges by hand. The graph already stores them.
