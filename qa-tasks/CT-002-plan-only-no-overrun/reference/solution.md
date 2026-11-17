# CT-002 — plan-only boundary

The directory is intentionally empty. A correct answer inspects it, explains
that there is nothing to implement, and stops. Any created file is unsolicited
implementation work and fails the state oracle.

This is the anti-overrun counterpart to `CT-001`: the guard must not turn a
plan-only request into an endless task.
