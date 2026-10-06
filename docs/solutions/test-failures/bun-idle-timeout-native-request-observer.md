---
title: Keep native request-event observers alive across consent latency
date: "2026-10-04"
category: test-failures
module: native-review-acceptance
problem_type: test_failure
component: testing_framework
severity: medium
symptoms:
  - "The loopback event subscriber prints CONNECTED, then curl exits 18 before a native consent decision."
  - "The application can still complete the approved request after the observer has already disconnected."
root_cause: config_error
resolution_type: config_change
tags: [native-acceptance, loopback, bun, idle-timeout, request-consent, event-subscription]
retire_when: "Bun no longer applies configured HTTP idle timeouts to quiet streaming responses; check its server API documentation and a bounded fixture observation."
---

# Keep native request-event observers alive across consent latency

## Problem

A quiet loopback request-event observer can disconnect during native consent
before the approval-triggered request it was supposed to prove. The product may
still work; broken observation means missing acceptance evidence, not a failed
product request.

## Symptoms

In the verified Goal 08 run, the event subscription emitted `CONNECTED`, then
`curl.exe` exited with code 18 before an approval-triggered request was observed.
A disconnected observer did not establish an application failure, cancellation,
or send without consent.

The quiet interval is legitimate: the native Review path rejects an unapproved
decision before continuing to revalidation and authorization
(`crates/mt-app/src/views/workspace/review.rs:2748-2806`). The harness must tolerate
that interval without shortening the product's approval boundary.

## What Didn't Work

The client already had a bounded 600-second `--max-time`. A longer client
deadline cannot prevent the server from closing a quiet connection earlier.
Reopening the observer without changing that setting leaves the same failure
mode: another quiet interval longer than the server idle timeout closes it again.
Counting only the initial connection, or treating a lost observer as a failed
product request, confuses instrumentation state with application behavior.

## Solution

For the owned, loopback-only Bun fixture used in the verified run, recreate the
server with its idle timeout disabled while retaining the bounded client wait:

```ts
// Configuration excerpt: reuse the fixture's existing port and handler.
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: fixturePort,
  idleTimeout: 0,
  fetch: handleFixtureRequest,
});
```

Keep the event subscriber bounded, with unbuffered, silent curl output so
newline-delimited event markers can be consumed directly:

```powershell
curl.exe -sS -N --max-time 600 "$FixtureEndpoint/events"
```

Register the observer before triggering native Run/Send. Establish that its
subscriber is connected, then perform the consent interaction. After the exact
request event, assert the actual request count and parsed selected-source
payload independently; a marker alone does not establish payload correctness.

The corrected run emitted `CONNECTED`, then `REQUEST_COUNT=1`, and the subscriber
exited 0. Viewing, selection and Cancel retained count 0; only the fresh approved
operation produced the single captured request. This was fixture recovery, not
a product transport change or a second model pipeline.

## Why This Works

[Bun's server documentation](https://bun.com/docs/runtime/http/server#idletimeout),
consulted on 2026-10-04, states that its default HTTP idle timeout is 10 seconds,
applies to quiet streamed responses, and that 0 disables it. The server's
inactivity limit and the client's total wait budget are separate controls:
removing the former for the observer does not make the latter unbounded.

For a fixture that needs ordinary idle limits on other routes, the documented
per-request alternative is `server.timeout(request, 0)` on the event route. That
API is a documented option, not a separately executed fix in this run. The
session-verified recovery was server-level `idleTimeout: 0` above.

This failure occurred in a temporary fixture held in session memory. Reading the
final MarkTurbo implementation or tests does not recover that fixture's timeout
setting or the distinction between observer failure and product failure.

## Prevention

- Keep this setting confined to the owned loopback fixture; do not weaken the
  product's transport timeout or Request Consent to repair observation tooling.
- Keep a total subscriber deadline even when server idle expiry is disabled.
- Subscribe to the actual request event before the native action; do not replace
  synchronization with sleeps, polling, or repeated reconnections.
- Separate observer connection, actual request count, and parsed payload checks.
- Treat observer truncation or timeout as missing evidence, never as acceptance.
- Close the owned server and subscriber after the case, including cancellation
  and environment loss; do not leave a quiet fixture running indefinitely.
- Keep credentials, document bodies, raw payloads and private local paths out of
  native evidence. Use content-free markers and equality results.

## Related Issues

- [Keep the test boundary honest](../../development.md#keep-the-test-boundary-honest)
  and [keep native work bounded](../../development.md#keep-native-work-bounded)
  define the project's existing validation and evidence cadence.
- [Request Consent and Outbound Scope](../../../CONCEPTS.md#request-consent)
  provide the existing domain vocabulary; an observer is test instrumentation,
  not another authorization or domain entity.
- [Archived Goal 08](../../goals/archive/08-explain-effective-agent-context.md)
  preserves the original acceptance contract, not proof from the folder name.

The observed failure/recovery and request receipt were recorded in the local
Goal 08 native report's `Current-artifact selected-context run on 2026-10-04`
section. The historical unsolicited source banner was a different, unreproduced
observation and is not the root cause documented here. No public issue or merged
PR is claimed for this temporary-fixture recovery.
