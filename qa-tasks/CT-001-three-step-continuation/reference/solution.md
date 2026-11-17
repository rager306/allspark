# CT-001 — three-step continuation

The requested outcome is three files, not the first file:

```sh
printf 'ONE\n' > one.txt
printf 'TWO\n' > two.txt
printf 'THREE\n' > three.txt
```

The oracle checks the final file state. A response that reports `one.txt`
but leaves the other two missing is a failure even if its final answer is
plausible. This is the minimal semantic probe for the reported stop-after-
partial-progress behavior without a goal or a structured plan.
