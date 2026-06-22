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

1M orders, lock-step, single client, WSL2, release.

### The big result: single-threaded run-to-completion eliminated the tail

The OUCH order path was moved **off** the async+worker design. `sessions.rs` is now a
**synchronous, blocking, run-to-completion** server: one thread owns a plain `OrderBook`
and does read → `match_order` → write **inline** — no tokio, no channels, no lock, so
there are zero thread handoffs or scheduling points on the hot path.

| metric | async + worker | single-threaded |
|---|---|---|
| `eng` p50 (match_order) | ~10µs | **201 ns** |
| **`eng` p99.9** | **~5.7 ms** | **~6 µs** (~1000×) |
| client p99 | ~3 ms | **83 µs** |
| client p99.9 | ~6.5 ms | **127 µs** |
| throughput | ~11k/s | **24.7k/s** (2.2×) |

So the entire multi-ms tail was the **cross-thread handoff + scheduler wakeup**
(crossbeam → worker thread → tokio mpsc → task wakeup) — **not** the matching, the
gateway, or the socket. Proven by the segmented `svc`/`eng`/`wr` histograms: the `eng`
(engine round-trip) segment carried ~100% of the old tail.

### The journey there (for reference)
- Segmented `svc`/`eng`/`wr` to localize → tail lived entirely in `eng`, exonerating gateway parse/encode and the socket.
- **Nagle/delayed-ACK** was a real bug — server socket wasn't `TCP_NODELAY` → ~36ms `wr` stalls. Fixed (`set_nodelay(true)`), carried into the single-threaded server.

### New bottleneck: the system is now I/O-bound, not compute-bound
- match = ~200ns (free); parse+match+encode (`svc`) ~1.3µs p50.
- **socket write (`wr`) ~13µs p50 is now the largest server-side cost** — it's the `write_all` syscall + TCP stack, not our code.
- client RTT ~37µs p50 = mostly client/server syscalls + loopback.
- Remaining tail (client p99.9 ~127µs) is syscall/socket jitter; the rare ms `max` is WSL2 descheduling (environmental).

Further latency now needs **OS/hardware** work (busy-polled sockets / `io_uring` / kernel-bypass, bare-metal Linux), not architecture.

> Architecture note: the async worker / `AppState` / slot-pool machinery still exists
> (REST `rest-gateway` uses it), but the **OUCH path no longer does** — it's the
> single-threaded blocking server in `sessions.rs`/`server` `main.rs`. Sections 2–3
> describe the original worker design; the OUCH hot path has diverged from it.

### Throughput (the 28M ops/sec goal) — separate axis, measured in-memory

Throughput ≠ latency. The lock-step network test is RTT-bound (~25k/s) and **cannot**
measure throughput. The real number comes from the in-memory bench
(`cargo bench -p matching-core throughput`, `benches/order_book.rs`), which feeds orders
straight to `match_order` with no network. Single-core results:

| bench | before | after reusable buffer | per-op |
|---|---|---|---|
| **`insert_warm`** (steady-state insert) | 25.4 M/s | **32–33 M/s** | 39 → **31 ns** ✅ **past 28M on one core** |
| `insert_no_match` (new level each op) | 14.6 M/s | **24 M/s** | 68 → 42 ns |
| `add_then_match` (with fills) | 2.6 M/s | **4.7 M/s** | 379 → 207 ns |

Match-path latency (in-memory): p50 ~0–100ns, **p99.9 ~101ns**, cancel p99.9 ~101ns.

**The win:** `match_order` allocated a `Vec<OrderEvent>` per call. Added `match_order_into(book, order, cmd, out: &mut Vec<OrderEvent>)` (clears + reuses the caller's buffer); kept `match_order` as an allocating wrapper for non-hot call sites. The bench and the single-threaded server hot path (`gateway::read` holds `out: Vec<u8>` + `ev_buf: Vec<OrderEvent>` reused across messages) use `match_order_into`. Removing the per-call alloc gave +27–83% throughput **and** dropped match p99.9 from 794ns → ~101ns.

**Status vs 28M:** met on one core for the **insert/cancel** path (the bulk of real flow) — sharding across pinned cores → 60M+. The **fill path** (`add_then_match`, 4.7 M/s) is the remaining gap. Next lever: **`PriceLevel` pooling** — stop `None`-ing + re-allocating a price level's `Vec<Order>` on full drain; reuse it. That closes both `insert_no_match`→`insert_warm` and the fill-path churn. Also pending: non-atomic `trade_id` (fires only on fills).

### Wire throughput (over OUCH) — separate from engine throughput

The 33M is the *engine* number; the *wire* (order-to-ack over the socket) is far lower
and **syscall-bound**. Two clients:
- `load_client` — lock-step (1 order in flight) → measures **latency** (throughput pinned at `1/RTT` ≈ 25k/s).
- `pipe_client` — pipelined with a bounded in-flight window `W` (2nd arg) → measures **wire throughput + latency-under-load**. Sweep `W` to trace the latency-vs-throughput curve.

Latency-vs-throughput sweep (1M orders, single connection, single-threaded server, WSL2):

| W | throughput | p50 | p99 | p99.9 |
|---|---|---|---|---|
| 1 | 24.4k/s | 37.6µs | 80µs | 125µs |
| **4** | **62.5k/s** | **34µs** | 110µs | 168µs |
| 16 | 63.3k/s | 189µs | 305µs | 424µs |

- **Knee at W≈4:** 2.5× throughput (24k→62k) at *unchanged* p50 (~34µs) — the pipe was just idle waiting on RTT.
- Past the knee (W=16): throughput flat (~63k), latency 5×'s → pure queuing, no gain.
- **Headline wire figure: ~62k orders/sec at p99 = 110µs** (order-to-ack, OUCH included).
- Saturation ~63k/s = single-thread + ~3 syscalls/order; the match is ~0ns. To lift it: **batch syscalls** (`recvmmsg`/`readv`, write many acks at once) or `io_uring`, and/or multi-connection + sharded server. Wire will land in hundreds-of-k to low-M even optimized — 28M is the in-memory number, a different axis.
- Methodology note: unbounded push (no `W`) gives meaningless seconds-scale latency (queue depth ÷ drain rate / coordinated omission) — always bound in-flight for latency-under-load.

---

## 9. Next steps (current plan)

The tail is **solved** (section 8) — the system is I/O-bound, not architecture-bound.
Options from here, in rough priority:

- **Bank the result.** Median ~37µs RTT, p99.9 ~127µs, no ms tail. Good baseline.
- **(Optional) push syscall latency** — busy-polled sockets / `io_uring` for single-digit-µs gains. Deep HFT territory, big effort, diminishing returns.
- **(Optional) bare-metal Linux** to confirm the rare ms `max` is WSL2, not code.
- **Correctness/coverage over latency:** finish Cancel/Modify/Replace edge cases (reject fidelity, side handling), multi-symbol, then Milestone B.

**Caveat for Milestone B (matched flow):** the single-threaded server is
*one-connection-at-a-time* (sequential `accept`). Multi-client + maker-side fan-out
will need either **non-blocking I/O (a poll loop) on the single thread** or the
**thread-per-core shard** model — *not* the async egress design (section 7), which was
built for the worker architecture the OUCH path has now left behind. Revisit section 7
in that light before starting B.

---

## 10. Incidental fixes already made

- `NEXT_SESSION_ID`: `const` → `static` (a `const` AtomicU64 was inlined per call, so every session got id `1`).
- `main.rs`: `exchange.register_symbol(symbol_registry.register(...))` — use the returned id so registry and senders stay in sync.
- `trader_id` in the Enter path: hardcoded `1` → `sess.session_id as u32`.

---

## 11. Milestones & roadmap

### Milestone A — first wire-to-wire number ✅ DONE
Measure Enter→Accept wire-to-wire for a single session, order rests (no cross).
- [x] Enter handler encodes `Accept`; session map populated on `Accepted`
- [x] Response written back to the socket
- [x] Server-side timestamping + hdrhistogram (`svc`/`eng`/`wr` split, per-interval reset)
- [x] Load client: login → N `Enter`s lock-step → client-side percentiles
- [x] First real numbers obtained

### Tail-latency optimization ✅ DONE
Goal was to cut the p99/p99.9 tail; it's eliminated.
- [x] Segmented latency (`svc`/`eng`/`wr`) → tail localized entirely to the engine round trip
- [x] `TCP_NODELAY` (Nagle/delayed-ACK fix)
- [x] Per-interval histogram reset (steady-state windows)
- [x] **Single-threaded run-to-completion** (no tokio/channels/lock on the OUCH path) → `eng` p99.9 ~6ms → ~6µs, client p99.9 ~6.5ms → 127µs, throughput 2.2×
- Result: tail gone; system now **I/O-bound** (socket/syscall), not architecture-bound. See section 8.

### Throughput — 28M ops/sec goal 🟡 MET ON INSERTS, fill path remains
Measured in-memory (`cargo bench -p matching-core throughput`), not over the network.
- [x] Reusable `Vec<OrderEvent>` buffer (`match_order_into`) — +27–83% throughput, match p99.9 794ns → ~101ns
- [x] **`insert_warm` = 32–33 M/s single-core (31 ns/op)** → 28M goal met for the insert/cancel path (the bulk of real flow); sharding → 60M+
- [ ] Fill path (`add_then_match` 4.7 M/s) — next lever: **`PriceLevel` pooling** (reuse drained levels instead of realloc) + non-atomic `trade_id`
- [ ] Thread-per-core sharding for total throughput beyond one core
See section 8 for the table.

### Milestone B — realistic matched-flow latency ⬜ NOT STARTED
Measure latency under real two-sided flow (orders that cross and fan out to two clients).
**Note:** the single-threaded OUCH server serves one connection at a time, so B now needs
non-blocking I/O (poll loop) on the single thread **or** thread-per-core sharding — *not*
the async egress design (section 7), which targeted the abandoned worker architecture.
- [ ] Multi-client I/O on the single-threaded server (or thread-per-core shard)
- [ ] Maker-side fill delivery (resting order filled by another client)
- [ ] Cancel/Modify/Replace verified end-to-end + edge cases
- [ ] Cancel-on-disconnect
- [ ] Two-sided (crossing) load generator

Caveat: solo/learning pace + WSL2 jitter apply.
