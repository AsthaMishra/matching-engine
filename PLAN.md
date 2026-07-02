# Plan / Roadmap

What's next for this project. Architecture and measurement details live in the per-crate READMEs;
this file is only the forward path.

Last updated: 2026-07-02.

---

## Status

**Built:** order book + matching (core), sharded worker runtime, OUCH login/heartbeat/session
lifecycle, Enter/Replace/Cancel/Modify over the synchronous slot path, latency instrumentation +
load client. Server registers `AAPL` at startup.

**Not built yet:** async egress (see below), maker-side fill delivery, cancel-on-disconnect,
`MassCancel`/`DOE`/`EOE`/`Query` (still `todo!()`).

**✅ Latency phase — done (2026-07-02):** single-threaded run-to-completion **io_uring** datapath
(`io_uring_session.rs`, replaces the worker+channel path for OUCH) solved the `svc` tail;
**SQPOLL** on both server and client (`load_client_io_uring`) cut wire-to-wire RTT to **~9.75 µs**
(from ~38 µs blocking); pipelined client (`load_client_pipeline`) sustains **~1.74 M orders/sec**;
hot-path timers gated behind the off-by-default **`metrics`** feature. Details in
[`doc/progress.md`](doc/progress.md). Remaining latency lever: bare-metal Linux run (§4).

---

## 1. ✅ DONE - localize the `svc` tail

**Solved.** The tail was the cross-thread handoff/wakeup, confirmed by segmented histograms. Fixed by
moving OUCH off the worker+channel design to the single-threaded run-to-completion io_uring loop:
`svc` p999 ~5.7 ms → ~6 µs (~1000×). SQPOLL then took RTT to ~9.75 µs. Original analysis kept below
for history.

Median is healthy (`svc` p50 ≈ 13 µs); the work is the tail. `TCP_NODELAY` already removed the
Nagle/delayed-ACK stall, so the remaining tail (`svc` p999 ≈ 5.7 ms) is the **engine round trip**.

Before optimizing, instrument `add_order`'s round trip into three sub-phases:
1. send → worker pickup (wait in the crossbeam channel),
2. worker `match_order` processing,
3. reply → `recv().await` (response wait + async task wakeup).

Expected culprit: **cross-thread wakeup** (parked worker / tokio scheduling), not match compute.
Likely fix: busy-poll the worker, or move to the async egress architecture below.

## 2. Async "egress" architecture

> ⚠️ **OBSOLETE (mechanism), 2026-07-02.** This was designed for the **sharded worker + channel**
> runtime, which the OUCH path no longer uses — it's now a single thread that owns the book and every
> connection (io_uring). There are no workers to drain and no per-slot replies, so the egress channel
> + distribution layer (2.2–2.5) don't apply: the single thread can write a maker-side fill straight
> into the target connection's `resp` buffer by `conn_id`. **Still relevant:** the *problem*
> (maker-side delivery, cancel-on-disconnect) and the **opaque `token`** idea (2.1) as the routing key
> back to a session. Redesign against the single-threaded loop, not this worker shape. Kept for history.

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

Measure latency under real two-sided flow (orders that cross and fan out to two sessions). In the
single-threaded io_uring loop this no longer needs the §2 worker egress — the thread owns every
connection and writes a maker-side fill directly into the other session's `resp` by `conn_id`.

- [ ] Opaque `token` through engine (still useful as the session routing key)
- [ ] ~~Egress channel out of workers~~ — obsolete (no workers; see §2)
- [ ] ~~Distribution layer + per-session outbound channels~~ — obsolete; the loop routes by `conn_id`
- [ ] Cancel/Modify/Replace verified end-to-end
- [ ] Cancel-on-disconnect
- [ ] Two-sided load generator (crossing flow)

## 4. Credibility / housekeeping

- [ ] **External order-to-ack measurement** - current number is loopback + software timestamps;
      a real NIC/switch path with external capture is the honest figure to publish.
- [x] **Single-threaded baseline** - done: the single-threaded run-to-completion path is now *the*
      OUCH datapath and beat the sharded worker on the tail (~1000× on `svc` p999). See §1.
- [ ] **Fix the stale `fuzz/` target** - it uses the old `OrderBook` API and doesn't build.
- [ ] Bare-metal Linux run (CPU pinning + `SCHED_FIFO`) to separate real tail from WSL2 jitter.
      **Now the top remaining latency lever** — both SQPOLL conversions are done on WSL2.

---

## Known invariant to watch

> ⚠️ **OBSOLETE for the OUCH path, 2026-07-02.** The io_uring datapath removed symbol indexing
> (one process = one book = one sender), so there is no `AppState.senders`/`symbol_registry` coupling
> on it. Still applies to `rest-gateway`, which keeps `AppState`.

`AppState.senders` is immutable after construction but `symbol_registry` is mutable. Holds today
because all symbols register at startup ("symbol_id in registry ⟹ sender exists", relied on by the
gateway's `unreachable!`). Breaks if symbols are ever registered dynamically.
