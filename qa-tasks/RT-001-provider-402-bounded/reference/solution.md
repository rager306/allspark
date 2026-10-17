# Reference: RT-001 provider-402-bounded

## Why the task is solvable
The provider stack classifies HTTP 402 with an `insufficient_credit(s)`
body as a payment error, not a transient fault (fix `2e79330c3`,
`in_stream_credit_frame_classifies_as_payment_required`): the run fails
fast with the payment error surfaced into the transcript
(`Provider requires payment (insufficient_credit, 402): ...`), verified
green end-to-end against the dist binary: 3.8s, exit 1, bounded mock
requests.

## Reference behavior
1. POST `{base}/v1/messages` -> 402 `{"type":"error","error":{...,"code":"insufficient_credits"}}`.
2. The client classifies payment-required; payment failures never ride
   the transient retry ladder (`provider_retry.rs`: payment_required
   settles at once; only the daemon failover chain may re-route).
3. The run exits nonzero within seconds, error text in the transcript.

## Oracle rationale
State oracle, not text matching of the answer: duration bound (no hang),
request-count bound (no retry loop), and the error marker in the
transcript. Note: on the daemonless print path `retry.maxRetries` is not
consulted for payment failures at all (they settle immediately) - the
request-count cap stays as the regression tripwire for a future "payment
retried as transient" regression.
