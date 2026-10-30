Hashline patches existing files. Each file: `[PATH#TAG]`, `TAG` required 4-hex snapshot. Get both with `edit` `action: "read"` (prints `[path#TAG]` then `N:TEXT` rows, and mints the tag) — copy that header verbatim. Numbers: original `LINE:TEXT`, never hunk-shifted.

<ops>
`PUT N.=M:` replace inclusive N–M with `+` body (`N.=N` for one line); `PUT N*:` replace block N.
`PUT <N:`/`PUT >N:` insert before/after N (`<1` head, `>$` tail). `PUT >N*:` insert after block N at sibling depth; inside, use `PUT >M:` at closer.
`CUT N.=M`/`CUT N*` delete and capture, optionally as `@name`.
`PUT <N @name`/`PUT >N @name` paste at gap (omit name for anonymous CUT); `PUT N.=M @name`/`PUT N* @name` paste over range/block (name REQUIRED). Register pastes have NO body; named registers persist across calls.
`REM` delete file; `MV DEST` rename after prior edits (quote spaced paths).
</ops>

<rules>
- `:` ops only: body rows `+TEXT` verbatim incl. indent; lone `+` blank. Literal `- item`/`+ item` → `+- item`/`++ item`. NEVER `-`/bare context. Body length independent of range; delete with CUT, not empty PUT.
- Touch displayed changed lines only; `…`, `..`, collapsed `N-M:` and out-of-window lines UNSEEN. Re-read first. Tight ranges: split nonadjacent changes; NEVER include keepers or start/end mid-expression/block. Pure addition uses gap PUT.
- `*` requires multi-line opener, NEVER closer/last/inner statement; use range/gap for one statement. Anchor first decorator/attribute/doc-comment to include it; standalone comments need explicit range.
- Markdown heading blocks run through deeper headings until next same/higher; after section `PUT >N*:`, end body with blank line.
- NEVER restyle unrelated code. After EVERY edit tag/numbers change: use the tag the edit response returned; stale tag/surprise → STOP, `action: "read"` the file again and start from the fresh tag.
</rules>

<example>
```
[greet.py#A1B2]
PUT 1*:
+@cache
+def greet(name):
+    print(name)
[PLAN.md#3C4D]
PUT >2:
+- task
```
Cross-file move: `CUT 1* @fn` in source, then `PUT <1 @fn` in destination section.
</example>
