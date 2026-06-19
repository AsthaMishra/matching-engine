# Project Context & Handoff

A working log so a fresh chat can pick up mid-stream. Covers the architecture, the
OUCH identity model, what's built vs. not, the latency-measurement setup, and the
findings/next-steps of the current tail-latency optimization effort.

Last updated: 2026-06-16.

---

## 1. What this project is

A from-scratch **matching engine** with a **NASDAQ OUCH/ITCH-style gateway**, in Rust.
Four workspace crates, layered **core ← runtime ← adapters** (deps point downward only):

- `matching-core/` — **pure core** (no threads/async/transport, dep = `serde` only): order books, matching, `types/`, `symbol_registry`, `response`/result types, `error`, `utils`. Also owns `benches/`, `tests/`, the itch replay `src/bin/`, and `fuzz/`.
- `matching-engine/` — **runtime + adapters**: per-symbol sharded workers (`matcher`, `exchange`), `app_state` (channels/slot pool), and the transport adapters (`routes` = REST, `ouch` = engine-facing OUCH bridge). Depends on `matching-core` and re-exports it (`pub use matching_core::*`), so `matching_engine::…` / `crate::…` paths still resolve everywhere.
- `ouch-gateway/` — the OUCH protocol layer: TCP sessions, login/heartbeat, inbound parsing, outbound encoding, the session order map.
- `server/` — the binary entry point; wires engine + gateway together and registers symbols.

> Refactor note (2026-06-16): the pure core was split out of `matching-engine` into
> `matching-core` so purity is compiler-enforced (the core crate can't take a thread/
> transport dep). The `fuzz/` target moved with it but is **stale** (old `OrderBook`
> API) and needs updating before it builds.

Run:
```bash
cargo run --release -p server                                   # terminal 1
cargo run --release -p ouch-gateway --bin load_client -- 1000000 # terminal 2 (load test)
```
Server binds `127.0.0.1:8080`. `RUST_LOG=info` (default) shows the latency logs.

**Goal of the current phase:** measure and reduce **wire-to-wire latency**, especially the tail. See **section 11** for milestone status.

> Environment note: development is on **WSL2**, which injects scheduler/VM jitter
> (100ms+ pauses). Part of the measured tail is environmental, not code. Bare-metal
> Linux would give cleaner tail numbers.

---

## 2. Engine architecture (matching-engine/)

- **Per-symbol single-threaded matching, sharded across worker threads.** No locks inside a book.
  - `run_worker` ([matching-engine/src/matcher.rs](matching-engine/src/matcher.rs)) owns a `Vec<SymbolSlot>`; each `SymbolSlot` = one `OrderBook` + one crossbeam `Receiver<BookRequest>`. A `crossbeam::Select` waits across all its symbols' channels plus a registration channel.
  - Sharding: `Exchange::register_symbol(symbol_id)` ([matching-engine/src/exchange.rs](matching-engine/src/exchange.rs)) picks a worker via `symbol_id % num_workers`, makes a `(tx, rx)` channel, sends `rx` to that worker's registration channel, and stores `tx` in `senders`.
- **`AppState`** ([matching-engine/src/app_state.rs](matching-engine/src/app_state.rs)), cloneable, holds:
  - `senders: Arc<HashMap<u32 symbol_id, Sender<BookRequest>>>` — **immutable** after construction.
  - `symbol_registery: Arc<RwLock<SymbolRegistry>>` — name `[u8;8]` → `symbol_id`, **mutable**.
  - `slot_pool: Arc<ArrayQueue<(usize slot_id, Receiver<BookResponse>)>>` — the response-slot pool (see below).
  - ⚠️ Latent inconsistency: `senders` is immutable but `symbol_registery` is mutable. Today all symbols are registered at startup, so "symbol_id in registry ⟹ sender exists" holds (the `unreachable!` in the gateway relies on it). If symbols are ever registered dynamically, that invariant breaks.
- **Slot/response mechanism (synchronous request/response).** `ouch::add_order` & friends ([matching-engine/src/ouch/order.rs](matching-engine/src/ouch/order.rs)) pop a `(slot_id, rx)` from `slot_pool`, send `BookRequest{slot_id}` to the symbol's worker, `rx.recv().await`, then push the slot back. The worker replies via `response_txs[slot_id]`. This is a REST-style pattern.

---

## 3. OUCH gateway (ouch-gateway/)

- [ouch-gateway/src/sessions.rs](ouch-gateway/src/sessions.rs) — `TcpListener`, per-connection `session()` task: login (`L`), then a `select!` loop over inbound packets + heartbeats + a metrics tick. Sequenced (`S`) packets go to `gateway::read`, whose bytes are written straight back to the socket.
- [ouch-gateway/src/gateway.rs](ouch-gateway/src/gateway.rs) — `read()`: parse inbound → dispatch (Enter/Replace/Cancel/Modify) → call the engine → encode response into a `Vec<u8>`.
- [ouch-gateway/src/codec/inbound.rs](ouch-gateway/src/codec/inbound.rs) — `parse_enter_order`: byte-level parsing of `O/U/X/M/C/D/E/Q`.
- [ouch-gateway/src/codec/outbound.rs](ouch-gateway/src/codec/outbound.rs) — encoders for `A/U/C/D/E/B/J/P/I/T/M/R/X/G/K/Q` (fixed-size byte arrays).
- [ouch-gateway/src/codec/types.rs](ouch-gateway/src/codec/types.rs) — `AddOrder`/`ReplaceOrder`/`CancelOrder`/`ModifyOrder` structs + their `write(OrderEvent) -> Vec<u8>` methods (map engine events → OUCH messages).

### The session order map (the key data structure)
```rust
struct Session {
    username: [u8; 6],
    session_id: u64,
    next_seq: u64,
    map: HashMap<u32 /*user_ref_num*/, OrderHandle>,
}
struct OrderHandle { sender, order_id /*engine id*/, symbol, capacity, cross_type, ci_ord_id }
```
Populated on `OrderEvent::Accepted`. Lets symbol-less Cancel/Modify/Replace (which carry only `user_ref_num`) resolve to the engine order id + symbol.

---

## 4. The OUCH identity model (important, easy to get wrong)

Three distinct ids, three owners, three jobs:

| id | owner | scope | purpose | changes per order? |
|---|---|---|---|---|
| **user_ref_num** | client | unique **per session** (protocol-enforced) | the handle for cancel/replace; the gateway's map key | **yes, always** |
| **order_ref** (engine `order_id` from `book.allocate_id()`) | exchange | unique **per book** | the engine's internal id; published on ITCH | n/a |
| **ci_ord_id** | client | free-form (exchange ignores) | client's own bookkeeping tag, echoed back verbatim | usually, but **may repeat** |

Consequences:
- `user_ref_num` is **not** an order id and is **not** the symbol selector — it's a per-order client token that the session map resolves to (symbol, engine id).
- The map is keyed by `user_ref_num` *because* it's unique per order. `ci_ord_id` must **not** be a key (it can collide).
- `add_order` only needs the symbol because Enter is where the order is born; Cancel/Modify/Replace don't carry symbol — the map supplies it.
- On the wire, `order_accepted` carries **both** `user_ref` and a separate `ord_ref_num` (the engine id) — keep them in their correct fields.

---

## 5. What's built vs. not

**Built (synchronous slot path):**
- Login / heartbeat / session lifecycle.
- Enter / Replace / Cancel / Modify handlers in `gateway::read`, session map populated on Accept.
- Inbound parsing + outbound encoders.
- Server registers `AAPL` at startup.
- Latency instrumentation + a load client (section 6).

**Not built:**
- The **async egress architecture** (section 7) — currently everything is synchronous request/response via the slot pool.
- **Maker-side fill delivery**: the sync path only replies to the *taker* who sent the order. A *resting* order filled by someone else's order has no in-flight request, so that client is never notified. This is the main functional gap; it needs the egress architecture.
- **Cancel-on-disconnect**, `MassCancel`/`DOE`/`EOE`/`Query` (still `todo!()`).
- REST API is **explicitly out of scope** — the goal is "everything the OUCH way." The `matching-engine/src/routes/` REST code exists but is not a target; don't let it constrain the OUCH design. (It's the only other user of the slot pool, so once OUCH goes async the slot mechanism can eventually be removed.)

---

## 6. Latency measurement setup

**Server-side** ([ouch-gateway/src/sessions.rs](ouch-gateway/src/sessions.rs)):
- `tracing` + `tracing-subscriber` for diagnostics (init in [server/src/main.rs](server/src/main.rs)); boundary logs only, nothing on the hot path.
- Two per-session `hdrhistogram` histograms, `new_with_bounds(1, 60_000_000_000, 3)`:
  - **`svc`** = `gateway::read` (parse + engine round trip + encode).
  - **`wr`** = the socket `write_all`.
  - Recorded in the `b'S'` arm via `t0/t1/t2` `Instant`s (no I/O on the hot path).
  - Reported every 5s **and reset each interval** (steady-state windows) + a final trailing-window report on close.

**Client** ([ouch-gateway/src/bin/load_client.rs](ouch-gateway/src/bin/load_client.rs)):
- Blocking `std::net::TcpStream`, `TCP_NODELAY` on. Login, then sends N `Enter` orders **lock-step** (send one, `read_exact(64)` the `Accept`, repeat), recording client-side round-trip latency into an hdrhistogram. Warmup = `min(N/10, 10000)`.
- Sends all-buy limit orders at the same price on `AAPL` → every order just rests → exactly one 64-byte `A` response (deterministic framing).
- Usage: `cargo run --release -p ouch-gateway --bin load_client -- <N>`.

**Methodology rules learned:**
- Always `--release` (debug latency is 3–10× and meaningless).
- This is a **latency** test, not throughput: lock-step throughput is RTT-bound (`1/round-trip`), ~12k/s here. For max throughput you'd pipeline (don't, for latency).
- `hdrhistogram`: use `new_with_bounds` (the bare `new(3)` + `saturating_record` clamps everything to a tiny initial ceiling — that bug gave us fake "2 ns" readings).

---

## 7. Planned async "egress" architecture (future, not built)

The clean end-state that fixes maker-side delivery and (likely) the cross-thread tail. Core principle: **the engine echoes an opaque token and emits one event stream; routing lives outside the core.**

1. **Opaque token through the engine.** Add `token: u64` to `PlaceOrder`/`Order`/every `OrderEvent`/`Trade` (maker+taker). The engine treats it as meaningless bytes and copies it onto every event. The gateway packs `token = (session_id << 32) | user_ref_num`.
   - High 32 bits → which session to route to. Low 32 bits → the `user_ref_num` (so the session decodes it directly — **no reverse `engine_id→user_ref` map needed**).
2. **One egress channel out of the workers.** The worker stops replying per-slot and instead emits `Egress { token, SessionEvent }` to a single channel. A crossing trade emits two egress items (taker leg + maker leg), each tagged with its owner's token.
3. **Distribution layer** (lives outside the core): owns `session_id → out_tx`, drains the egress stream, routes each event by `token >> 32`. Uses `try_send` (never block the worker).
4. **Per-session outbound channel**: session registers `out_tx` on login, `select!`s on `out_rx` to write to its socket, deregisters on disconnect.
5. `ouch::add_order` becomes **fire-and-forget** (no slot, no await); responses arrive via egress.

Then add **cancel-on-disconnect** (resting orders of a dropped session must be cancelled, or their fills route to a missing session).

The matching core stays pure (`commands → event stream`); gateways/market-data are subscribers. This is the LMAX-Disruptor / NASDAQ-INET shape.

---

## 8. Latency findings so far

Baseline at 1M orders, lock-step, single session, WSL2, release:

- **Median is healthy & stable:** `svc` p50 ≈ 13µs, `wr` p50 ≈ 10µs, client round-trip p50 ≈ 33µs.
- **Tail investigation (segmented svc vs wr):**
  - **Nagle/delayed-ACK was a real bug.** The server's accepted socket wasn't setting `TCP_NODELAY` (only the client was) → periodic ~36ms `wr` stalls. **Fixed** with `stream.set_nodelay(true)` in `session()`. Result: `wr_p999` ~3–4ms → ~130µs; client p99 2.96ms → 1.81ms; throughput +12%.
  - **WSL2 jitter** causes co-occurring 100ms+ maxes in *both* segments (whole-process descheduling). Environmental, not code.
  - **Remaining tail is in `svc`** (`p999` ~5.7ms cumulative) = the **engine round trip** (crossbeam send → worker thread wakeup → tokio mpsc reply → task wakeup). This is the next target.

**Gateway-side tail work is essentially done** — Nagle was the one real lever. Allocation/clone micro-opts (per-order `Vec`s, `state.clone()`) are hygiene and will **not** move a multi-ms tail, so they're deprioritized.

---

## 9. Next steps (current plan)

**Just completed:** per-interval histogram reset (steady-state windows). Next run: read the *middle* windows (ignore window 1 = cold start) to see the true steady-state `svc` tail.

**Engine phase (next):** localize the `svc` tail before optimizing. Instrument `ouch::add_order`'s round trip into three sub-phases:
1. send → worker pickup (time the request waits in the crossbeam channel),
2. worker `match_order` processing,
3. reply → `recv().await` (response wait + async task wakeup).

Expected culprit: **cross-thread wakeup latency** (parked worker thread / tokio scheduling), not match compute. Likely fixes: busy-poll the worker, or move to the async egress architecture (section 7).

---

## 10. Incidental fixes already made

- `NEXT_SESSION_ID`: `const` → `static` (a `const` AtomicU64 was inlined per call, so every session got id `1`).
- `main.rs`: `exchange.register_symbol(symbol_registry.register(...))` — use the returned id so registry and senders stay in sync.
- `trader_id` in the Enter path: hardcoded `1` → `sess.session_id as u32`.

---

## 11. Milestones & roadmap

Two milestones drive the latency goal, plus the current optimization sub-phase.

### Milestone A — first wire-to-wire number ✅ DONE
Measure Enter→Accept wire-to-wire for a single session, order rests (no cross).
Achieved via the **synchronous slot path** (the async egress design was deferred to B).
- [x] Enter handler encodes `Accept`; session map populated on `Accepted`
- [x] Response written back to the socket
- [x] Server-side timestamping + hdrhistogram (`svc`/`wr` split, per-interval reset)
- [x] Load client: login → N `Enter`s lock-step → client-side percentiles
- [x] First real numbers obtained (median ≈ 13µs `svc` / ≈ 33µs client round-trip)

Original estimate: ~4–6 focused days. Done.

### Milestone B — realistic matched-flow latency ⬜ NOT STARTED
Measure latency under real two-sided flow (orders that cross and fan out to two sessions).
**Requires the async egress architecture (section 7)** — the synchronous path structurally
cannot deliver maker-side fills.
- [ ] Opaque `token` through engine (`PlaceOrder`/`Order`/`OrderEvent`/`Trade`)
- [ ] Egress channel out of workers; worker emits instead of slot-reply
- [ ] Distribution layer (`token >> 32` → session) + per-session outbound channels
- [ ] `add_order` fire-and-forget; Cancel/Modify/Replace verified end-to-end
- [ ] Cancel-on-disconnect
- [ ] Two-sided load generator (crossing flow)

Estimate: ~5–8 focused days on top of A.

### Current sub-phase — tail-latency optimization 🔻 IN PROGRESS
Sits on top of Milestone A's measurement; goal is to cut the p99/p99.9 tail.
- [x] Gateway: `TCP_NODELAY` (Nagle/delayed-ACK fix) — `wr` tail ~ms → ~130µs, client p99 2.96ms → 1.81ms
- [x] Per-interval histogram reset (steady-state windows)
- [ ] Engine: instrument `ouch::add_order` round trip into 3 sub-phases to localize the ~5.7ms `svc` tail
- [ ] Optimize the located cause (likely cross-thread wakeup; possibly resolved by section 7's async design)

See sections 8–9 for findings & plan. Caveat: solo/learning pace + WSL2 jitter inflate both the schedule and the tail.

---

## 11. Milestones & roadmap

Two milestones drive the latency goal, plus the current optimization sub-phase.

### Milestone A — first wire-to-wire number ✅ DONE
Measure Enter→Accept wire-to-wire for a single session, order rests (no cross).
Achieved via the **synchronous slot path** (the async egress design was deferred to B).
- [x] Enter handler encodes `Accept`; session map populated on `Accepted`
- [x] Response written back to the socket
- [x] Server-side timestamping + hdrhistogram (`svc`/`wr` split, per-interval reset)
- [x] Load client: login → N `Enter`s lock-step → client-side percentiles
- [x] First real numbers obtained (median ≈ 13µs `svc` / ≈ 33µs client round-trip)

Original estimate: ~4–6 focused days. Done.

### Milestone B — realistic matched-flow latency ⬜ NOT STARTED
Measure latency under real two-sided flow (orders that cross and fan out to two sessions).
**Requires the async egress architecture (section 7)** — the synchronous path structurally
cannot deliver maker-side fills.
- [ ] Opaque `token` through engine (`PlaceOrder`/`Order`/`OrderEvent`/`Trade`)
- [ ] Egress channel out of workers; worker emits instead of slot-reply
- [ ] Distribution layer (`token >> 32` → session) + per-session outbound channels
- [ ] `add_order` fire-and-forget; Cancel/Modify/Replace verified end-to-end
- [ ] Cancel-on-disconnect
- [ ] Two-sided load generator (crossing flow)

Estimate: ~5–8 focused days on top of A.

### Current sub-phase — tail-latency optimization 🔻 IN PROGRESS
Sits on top of Milestone A's measurement; goal is to cut the p99/p99.9 tail.
- [x] Gateway: `TCP_NODELAY` (Nagle/delayed-ACK fix) — `wr` tail ~ms → ~130µs, client p99 2.96ms → 1.81ms
- [x] Per-interval histogram reset (steady-state windows)
- [ ] Engine: instrument `ouch::add_order` round trip into 3 sub-phases to localize the ~5.7ms `svc` tail
- [ ] Optimize the located cause (likely cross-thread wakeup; possibly resolved by section 7's async design)

See sections 8–9 for findings & plan. Caveat: solo/learning pace + WSL2 jitter inflate both the schedule and the tail.
