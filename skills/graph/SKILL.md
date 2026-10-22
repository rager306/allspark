---
name: graph
description: Route an architectural question to the right graph action. Use when unsure which of trace/search/architecture answers "who calls X", "what breaks if I change Y", or "how is the code organized" - it returns the ready-to-issue tool call with the correct direction and depth.
---

# Graph router

The native `graph` tool executes queries over the project's symbol/call/import
graph. This skill routes a plain-language question to the right action, because
picking the wrong direction (`in` vs `out`) or the wrong action (`search` vs
`trace`) costs a round-trip and teaches you the tool "did not work".

Route a question, then issue the returned call yourself:

    from graph import route, format_call
    call = route("who calls load_config")
    # -> {"action": "trace", "symbol": "load_config", "direction": "in", "why": "..."}
    print(format_call(call))

    # Then call the native graph tool with those parameters.

Only the parameters listed in the graph tool's schema are accepted. Drop the
`why` note (it is a comment for you, not a tool argument): the tool rejects any
key it does not read, naming the accepted set in the error. That is deliberate -
a misspelt key used to fall back to the default and answer as if the call had
worked.

## The routing table

| The question is about | Action | Why |
|---|---|---|
| who calls X, who uses X | `trace`, `direction: in` | incoming edges reach the symbol |
| what does X call, blast radius of X | `trace`, `direction: out` | outgoing edges are what breaks |
| a chain from A to B | `trace`, both directions, deeper | two known endpoints |
| cycles, layers, hotspots, entry points, structure | `architecture` with `view` | shapes, not symbols |
| where is the code that does X | `search` with plain words | discovery; hybrid BM25+degree beats a regex |

The graph indexes itself on first use (`search`, `trace`, and `architecture`
build it when empty), so there is no separate index step.
