# Retry execution

`relentless` repeats a caller's operation until a classifier accepts an outcome, aborts on one, or a stop strategy gives up. This context names the parts of one retry execution and the policy that configures it, for sync and async callers alike.

## Language

### The execution

**Operation**:
The caller's function or closure that one attempt runs once.
_Avoid_: op (outside type names), probe, task, job

**Attempt**:
One 1-indexed run of the operation inside an execution.
_Avoid_: try, call, iteration

**Outcome**:
The whole value one attempt produces, before classification.
_Avoid_: result (reserved for `Result`), response

**Retry execution**:
One run of the retry loop from `.call()` until it terminates.
_Avoid_: retry loop (acceptable informally), run, retry (as a noun)

**Retry policy**:
A reusable set of stop strategy, wait strategy, classifier and timeout, without an operation.
_Avoid_: config, retry rules

**Builder**:
The value that pairs one operation with a policy's settings plus hooks, ready for `.call()`.
_Avoid_: retry (as a noun)

### Classification

**Classifier**:
The rule that sorts each outcome into a verdict.
_Avoid_: decider, predicate (a predicate is one input to a classifier)

**Verdict**:
A classifier's three-way judgment on one outcome: return, retry or abort.
_Avoid_: decision (see Flagged ambiguities), fate

**Decision**:
A two-way verdict with no abort arm: return or retry.
_Avoid_: using "decision" for the three-way form

**Return** (verdict):
The verdict that ends the execution with a value for the caller.
_Avoid_: accept, success, done

**Retry** (verdict):
The verdict that hands the outcome back so another attempt may run.
_Avoid_: continue, keep going

**Abort** (verdict):
The verdict that ends the execution with a fatal payload.
_Avoid_: reject, fail, fatal (as a noun)

**Abort payload**:
The value an abort verdict projects for the caller; the bare error on the `Result` path.
_Avoid_: last (see Flagged ambiguities)

**Predicate**:
A boolean test on a `Result` outcome that `.when` or `.until` turns into a classifier.
_Avoid_: condition, filter

**Self-classifying outcome**:
An outcome type that implements `Outcome` and so needs no classifier at the call site.

### Pacing and stopping

**Wait strategy**:
The rule that computes the delay before the next attempt.
_Avoid_: backoff (one family of wait strategies), sleep strategy

**Delay**:
The duration the execution pauses between two attempts.
_Avoid_: wait (as a noun for the duration), sleep, backoff interval

**Jitter**:
Randomness applied to a delay to spread concurrent callers apart.

**Stop strategy**:
The rule that decides, after a retry verdict, whether to give up.
_Avoid_: limit, retry budget

**Timeout**:
A deadline on elapsed time for the whole execution, which also clamps the final delay.
_Avoid_: deadline, budget (both used as synonyms in SPEC and source)

**Elapsed time**:
Clock time since the execution's first attempt began.
_Avoid_: duration, wall time (the injected clock need not be the wall clock)

**Clock**:
The injected source of both elapsed time and delays.
_Avoid_: sleeper, timer

### Termination and observation

**Termination**:
The end of an execution: returned, aborted or exhausted.
_Avoid_: stop (reserved for the stop strategy), exit (reserved for the hook view)

**Exhaustion**:
Termination because the stop strategy or timeout fired while the classifier still wanted to retry.
_Avoid_: giving up, timed out

**Hook**:
A caller callback that observes the execution at a fixed point without changing it.
_Avoid_: listener, callback, observer

**Retry stats**:
The summary of one execution: attempts, total elapsed time, total delay and termination reason.
_Avoid_: metrics

## Event model

Scenario walked: poll a job with `.decide`, `stop::attempts(20)`, exponential wait, `.timeout(30s)`, `before_attempt`, `after_attempt` and `on_exit` hooks. A second scenario walked the `RetryPolicy` reuse path.

| Command | Events | Standard name | Code |
|---|---|---|---|
| Configure a retry policy | | tenacity `Retrying(...)`; Polly `ResiliencePipeline` | `RetryPolicy::new`, `RetryPolicy::{stop, wait, timeout, when, until, decide}` |
| Erase policy strategy types | | | `RetryPolicy::{boxed, boxed_local}` |
| Bind an operation | | tenacity `@retry`; backon `.retry()` | `retry`, `retry_async`, `RetryExt::retry`, `AsyncRetryExt::retry_async`, `RetryPolicy::{retry, retry_async}` |
| Set the classifier | | tenacity `retry=` | `.decide`, `.when`, `.until` (builder and policy) |
| Build a predicate | | tenacity `retry_if_exception`, `retry_if_result` | `predicate::{any_error, error, ok, result}` |
| Set the wait strategy | | tenacity `wait=`; backon `Backoff` | `.wait`, `wait::{fixed, linear, exponential, decorrelated_jitter}`, `WaitExponential::base` |
| Compose wait strategies | | tenacity `wait_a + wait_b`, `wait_chain` | `Wait::{add, chain, cap}`, `+` |
| Add jitter | | AWS "Full Jitter", "Equal Jitter", "Decorrelated Jitter" | `Wait::{jitter, full_jitter, equal_jitter}`, `Jittered::{with_seed, with_nonce}` |
| Set the stop strategy | | tenacity `stop=`; gRPC `maxAttempts` | `.stop`, `stop::{attempts, elapsed, never}` |
| Compose stop strategies | | tenacity `stop_a \| stop_b` | `Stop::{or, and}`, `\|`, `&` |
| Set a timeout | | tenacity `stop_after_delay` (partial) | `.timeout` |
| Inject a clock | | | `.clock`, `TokioClock::new`, `EmbassyClock::new`, `FuturesTimerClock::new`, `GlooClock::with_now`, `VirtualClock::new` |
| Register a hook | | tenacity `before=`, `after=`, `retry_error_callback` | `.before_attempt`, `.after_attempt`, `.on_exit` |
| Request stats | | tenacity `.statistics` | `.with_stats` |
| Run the execution | Execution started | | `.call()` (sync, async, `*WithStats`) |
| | Attempt starting | tenacity `before` | `BeforeAttemptHook`, `RetryState` |
| Run an attempt | Attempt completed | tenacity `after` | `RetryOp::call_op`, `AttemptHook`, `AttemptState` |
| Classify the outcome | Outcome returned; outcome retried; outcome aborted | | `Outcome::classify`, `Predicate::should_retry`, `Verdict::{Return, Retry, Abort}`, `Decision::{Return, Retry}` |
| Check the stop strategy | Stop strategy fired; timeout reached | tenacity `stop` | `Stop::should_stop` |
| Compute the next delay | Delay computed; delay clamped to timeout | tenacity `wait` | `Wait::next_wait` |
| Pause for the delay | Delay elapsed | | `SyncClock::wait`, `AsyncClock::wait_async` |
| Read elapsed time | | | `Clock::now` |
| | Execution returned | | `Exit::Returned`, `StopReason::Returned`, `Ok(R)` |
| | Execution aborted | | `Exit::Aborted`, `StopReason::Aborted`, `RetryError::Aborted` |
| | Execution exhausted | tenacity `RetryError` | `Exit::Exhausted`, `StopReason::Exhausted`, `RetryError::Exhausted` |
| | Execution terminated (any) | tenacity `retry_error_callback` | `ExitHook`, `Exit::{attempt, elapsed, stop_reason}` |
| Inspect the failure | | | `RetryError::{stop_reason, last, into_last, last_error, into_last_error}` |
| Advance a virtual clock | Virtual time advanced | | `VirtualClock::{advance, waits}` |
| Build a test state | | | `RetryState::{for_attempt, with_elapsed, with_previous_delay}` |

Automations (event → command):

- Attempt completed → classify the outcome.
- Outcome retried → check the stop strategy.
- Stop strategy not fired → compute the next delay → pause for the delay → run an attempt.
- Timeout reached → clamp the delay to the remaining timeout; the stop check then fires on the next completed attempt.
- Execution returned / aborted / exhausted → fire `on_exit` hooks.

Views:

- **Retry state** (`RetryState`): attempt number, elapsed time, previous delay; read by the operation, stop strategy, wait strategy and `before_attempt`.
- **Attempt state** (`AttemptState`): attempt number, elapsed time, borrowed outcome; read by `after_attempt`.
- **Exit** (`Exit`): what the caller receives plus counters; read by `on_exit`.
- **Retry stats** (`RetryStats`): read by the caller after `.with_stats().call()`.

Unmapped (plumbing, no domain verb): `Decide`, `IntoDecision`, `DefaultClassifier`, `ClosureClassifier`, `When`, `Until`, `HookChain`, `StatelessOp`, `AsyncRetryOp`, `AsyncRun`, `DropStats`, `VirtualWait`, combinator structs (`WaitCapped`, `WaitChain`, `WaitCombine`, `StopAny`, `StopAll`, `Wait*`/`Stop*` strategy structs, `Predicate*` structs), `RetryResult`, `SystemClock`, `prelude`.

## Relationships

- A **Retry policy** holds exactly one **Stop strategy**, one **Wait strategy**, one **Classifier** and at most one **Timeout**.
- A **Retry policy** configures zero or more **Builders**; each **Builder** binds exactly one **Operation**.
- A **Builder** holds exactly one **Clock** and zero or more **Hooks** per hook point.
- A **Retry execution** runs one or more **Attempts**; each **Attempt** produces exactly one **Outcome**.
- The **Classifier** turns each **Outcome** into exactly one **Verdict**.
- A **Retry** verdict leads to either another **Attempt** (after one **Delay**) or **Exhaustion**.
- A **Return** or **Abort** verdict ends the **Retry execution** immediately; the **Stop strategy** is not consulted.
- A **Timeout** acts as an extra **Stop strategy** (OR'd in) and clamps the final **Delay**.
- The **Clock** supplies both **Elapsed time** and every **Delay**.
- Each **Retry execution** ends in exactly one **Termination** and produces one **Retry stats**.

## Example dialogue

> **Dev:** "If the job endpoint returns `Pending`, is that an **Outcome** the **Classifier** sees, or an error?"
> **Domain expert:** "An **Outcome**. With `.decide` the **Classifier** gives it a **Retry** verdict; nothing has to be an error for the loop to continue."
> **Dev:** "And when the job is `Failed`?"
> **Domain expert:** "**Abort** verdict. The **Abort payload** is the failure reason, and the **Stop strategy** never runs: an abort ends the **Retry execution** at once."
> **Dev:** "Suppose it stays `Pending` for 30 seconds and the **Timeout** is 30 seconds. Is that a separate termination?"
> **Domain expert:** "No. A **Timeout** is an extra stop condition, so it ends in **Exhaustion**, the same as running out of **Attempts**. The **Delay** before the final **Attempt** is clamped so that attempt starts at the deadline."
> **Dev:** "So `RetryStats` can't tell me whether the timeout or the attempt limit fired?"
> **Domain expert:** "Not directly. Compare total **Elapsed time** with the **Timeout**. That gap is flagged below."

## Flagged ambiguities

1. **"wait" names three things.** `Wait` is the wait strategy (computes a delay); `SyncClock::wait` / `AsyncClock::wait_async` perform the pause; `AsyncClock::Wait` is the pause future; `VirtualClock::waits()` records delays; `RetryStats::total_wait` sums delays while `RetryState::previous_delay` names one. Proposed: **Delay** for the duration, **Wait strategy** for the rule, **pause** (or keep `sleep`) for the clock action. Rename `total_wait` → `total_delay` and `VirtualClock::waits` → `delays`. Open: names are public API.
2. **"decision" vs "verdict".** `Decision` is the two-way enum, `Verdict` the three-way enum, `.decide` / `Decide::decide` produce either, and `IntoDecision::into_verdict` converts to a `Verdict`. "Decision" is both a type and the generic word. Proposed: keep both types; reserve "verdict" for the generic concept in prose; rename hidden `IntoDecision` → `IntoVerdict`. Open.
3. **`last` means two things on `RetryError`.** `Aborted { last: A }` holds the abort payload, but `RetryError::last()` returns `None` for `Aborted`. A field and an accessor with one name disagree. Proposed: rename the `Aborted` field to `payload` (or `reason`). Open: breaking change.
4. **`StopReason` covers non-stop terminations.** `StopReason::Returned` names a success, which the stop strategy never decided. "Stop" means both the stop strategy and any termination. Proposed: **Termination** for the concept; consider `TerminationReason` or `ExitReason`. Open.
5. **Timeout vs deadline vs budget.** SPEC and source use all three for one value. Resolved in this glossary: **Timeout**. Prose fix only.
6. **Timeout exhaustion is indistinguishable from attempt exhaustion** (SPEC 11.4.4). Not a naming problem, but the model has a "Timeout reached" event the code does not surface. Open: add `StopReason::TimedOut`?
7. **Hook names mix patterns.** `before_attempt` / `after_attempt` / `on_exit`; traits `BeforeAttemptHook` / `AttemptHook` / `ExitHook`. `AttemptHook` drops "After". Proposed: `AfterAttemptHook`. Open.
8. **Two state views with unparallel names.** `RetryState` (pre-attempt) and `AttemptState` (post-attempt, with outcome). Both describe an attempt. Resolved in glossary as **Retry state** / **Attempt state**; rename not proposed.
9. **Operation vs op vs probe.** Source and ADRs use all three. Resolved: **Operation**; "probe" stays only for the README "inverted probe" pattern.
10. **Return vs accept vs success.** README and `Predicate` docs say "accept"; enums say `Return`. Resolved: **Return**.
11. **Abort vs reject vs fatal.** `RetryError::Aborted` doc says "rejected an outcome as fatal". Resolved: **Abort**; ADR-0006 mentions a retired `RetryError::Rejected`.
