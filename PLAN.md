# Plan / Roadmap

What's next for this project. Architecture and measurement details live in the per-crate READMEs;
this file is only the forward path.

Last updated: 2026-06-24.

---

## Status

**Built:** order book + matching (core), sharded worker runtime, OUCH login/heartbeat/session
lifecycle, Enter/Replace/Cancel/Modify over the synchronous slot path, latency instrumentation +
load client. Server registers `AAPL` at startup.

**Not built yet:** async egress (see below), maker-side fill delivery, cancel-on-disconnect,
`MassCancel`/`DOE`/`EOE`/`Query` (still `todo!()`).

---

## 1. Next - localize the `svc` tail

Median is healthy (`svc` p50 ≈ 13 µs); the work is the tail. `TCP_NODELAY` already removed the
Nagle/delayed-ACK stall, so the remaining tail (`svc` p999 ≈ 5.7 ms) is the **engine round trip**.

Before optimizing, instrument `add_order`'s round trip into three sub-phases:
1. send → worker pickup (wait in the crossbeam channel),
2. worker `match_order` processing,
3. reply → `recv().await` (response wait + async task wakeup).

Expected culprit: **cross-thread wakeup** (parked worker / tokio scheduling), not match compute.
Likely fix: busy-poll the worker, or move to the async egress architecture below.

## 2. Async "egress" architecture

The end-state that fixes maker-side delivery and (likely) the cross-thread tail.
Core principle: **the engine echoes an opaque token and emits one event stream; routing lives outside the core.**

1. **Opaque token through the engine.** Add `token: u64` to `PlaceOrder`/`Order`/every
   `OrderEvent`/`Trade`. The engine copies it onto every event, treating it as meaningless bytes.
   The gateway packs `token = (session_id << 32) | user_ref_num` - high 32 bits route to a session,
   low 32 bits decode directly to `user_ref_num` (no reverse `engine_id → user_ref` map needed).
2. **One egress channel out of the workers.** Workers emit `Egress { token, SessionEvent }` instead
   of replying per-slot. A crossing trade emits two items (taker leg + maker leg).
3. **Distribution layer** (outside the core): owns `session_id → out_tx`, drains egress, routes by
   `token >> 32`, uses `try_send` (never block a worker).
4. **Per-session outbound channel:** session registers `out_tx` on login, `select!`s on `out_rx` to
   write its socket, deregisters on disconnect.
5. `add_order` becomes **fire-and-forget**; responses arrive via egress. The slot pool can then be removed.

Then add **cancel-on-disconnect** (a dropped session's resting orders must be cancelled, else their
fills route to a missing session). Shape: LMAX-Disruptor / NASDAQ-INET (commands → event stream;
gateways/market-data are subscribers).

## 3. Milestone - matched-flow latency

Measure latency under real two-sided flow (orders that cross and fan out to two sessions).
**Requires the async egress architecture above** - the synchronous path structurally cannot deliver
maker-side fills.

- [ ] Opaque `token` through engine
- [ ] Egress channel out of workers
- [ ] Distribution layer + per-session outbound channels
- [ ] `add_order` fire-and-forget; Cancel/Modify/Replace verified end-to-end
- [ ] Cancel-on-disconnect
- [ ] Two-sided load generator (crossing flow)

## 4. Credibility / housekeeping

- [ ] **External order-to-ack measurement** - current number is loopback + software timestamps;
      a real NIC/switch path with external capture is the honest figure to publish.
- [ ] **Single-threaded baseline** - benchmark the sharded runtime against a single-threaded build to
      quantify (or disprove) the win. Not yet done.
- [ ] **Fix the stale `fuzz/` target** - it uses the old `OrderBook` API and doesn't build.
- [ ] Bare-metal Linux run (CPU pinning + `SCHED_FIFO`) to separate real tail from WSL2 jitter.

---

## Known invariant to watch

`AppState.senders` is immutable after construction but `symbol_registry` is mutable. Holds today
because all symbols register at startup ("symbol_id in registry ⟹ sender exists", relied on by the
gateway's `unreachable!`). Breaks if symbols are ever registered dynamically.
