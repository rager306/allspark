# Reference

`main -> handle -> Repo::load -> 41 -> validate -> 42` (and `main -> Repo::new` first).

The order is the trap. `handle` calls `validate(repo.load())`, and Rust evaluates the ARGUMENT
before the call, so `Repo::load` returns 41 before `validate` ever runs. Reading the source
top-to-bottom suggests `validate` then `load`; only following the edges in evaluation order
gives `load` then `validate`.

The chain crosses three files, so answering it by reading files means opening all three and
reconstructing the edges by hand. The graph already stores them — and `graph trace` reports
them in traversal order, which is why the answer should come from the tool rather than from a
visual scan of the text.
