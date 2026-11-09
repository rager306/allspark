# Reference

The README says the replacement has been in production for eight months, it covers
every endpoint, and the last two callers were migrated in release 24.9, which is
deployed everywhere. The evidence points one way, so the typed call is expected to
return a HIGH probability of yes and `decision.txt` must hold `DELETE`.

What is graded is the AGREEMENT: `decision.txt` is compared with the probability the
jev call returned. An agent that answers DELETE without asking, or that gets a low
probability and writes DELETE anyway, fails the task — the instruction was to act on
the answer, and the oracle holds us to it in both directions (`p >= 0.5` -> DELETE,
`p < 0.5` -> KEEP).
