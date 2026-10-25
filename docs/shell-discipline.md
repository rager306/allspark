# Shell discipline: `rg` and `sd`, never `grep` and `sed`

This is a hard rule, requested by the maintainer and violated repeatedly. It is recorded here
because a rule that lives only in a prompt note dies on the next clone.

## The rule

In any shell command, **never** use `grep` or `sed`. Use `rg` (ripgrep) and `sd` (sd 1.0.0).

## Replacements

| instead of | use |
|---|---|
| `grep PATTERN FILE` | `rg PATTERN FILE` |
| `grep -r X DIR` | `rg X DIR` |
| `grep -v X` | `rg -v X` |
| `grep -c` | `rg -c` |
| `find … \| grep -v node_modules` | `rg --files -g '*.rs'`, or Python `rglob` with a SKIP set |
| `sed -n '10,20p' FILE` | Python: `Path(f).read_text().splitlines()[9:20]` |
| `sed -n '/start/,/end/p' FILE` | Python: slice between the two indices |
| `sed -i 's/a/b/' FILE` | `sd 'a' 'b' FILE`, or the `edit` tool with exact old/new strings |

## The "it is only reading" excuse is not an exception

`sed -n` for printing a line range is still `sed`. Read the file in Python and slice it; that also
keeps the content in a variable so it can be inspected again without re-reading.

## Why this matters beyond taste

Editing a file with a regex has already cost us: a `sed`-style rewrite spliced a doc block into an
unrelated test and landed in a commit that broke workspace clippy. `edit` with an exact unique
`old_str` fails loudly instead of silently matching the wrong place.

## Check before every bash call

The command string must not contain the substrings `grep` or `sed `.
If the command needs either, it needs Python (`pathlib` + slicing) instead.
