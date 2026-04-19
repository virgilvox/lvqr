//! In-process AI agents framework for LVQR.
//!
//! **Tier 4 item 4.5, session A.** This is the scaffold crate
//! referenced by `tracking/TIER_4_PLAN.md` section 4.5. The
//! goal is to give per-broadcast in-process consumers a uniform
//! seat at the [`lvqr_fragment::FragmentBroadcasterRegistry`]
//! table without each agent needing to re-derive the
//! callback / spawn / drain / panic-isolate boilerplate that
//! every existing consumer (HLS bridge, archive indexer, WASM
//! filter tap, cluster claim) already encodes by hand.
//!
//! # Surface
//!
//! * [`Agent`]: the trait every concrete agent implements. One
//!   instance per `(broadcast, track, factory)` triple. The
//!   trait is sync; agents that want async or blocking work
//!   spawn from inside [`Agent::on_start`] (typical pattern: a
//!   bounded `tokio::sync::mpsc` to a worker task that owns
//!   the heavy state, e.g. a `whisper-rs` model handle).
//! * [`AgentContext`]: snapshot of the
//!   `(broadcast, track, FragmentMeta)` triple a fresh agent
//!   sees at construction time.
//! * [`AgentFactory`]: the trait an agent type registers under.
//!   Decides whether a new `(broadcast, track)` should get an
//!   instance of this agent type at all (returns `None` to
//!   skip), and otherwise builds the concrete agent.
//! * [`AgentRunner`]: the registry-side installer. Holds N
//!   factories, wires one
//!   [`lvqr_fragment::FragmentBroadcasterRegistry::on_entry_created`]
//!   callback that, for every new broadcaster, asks every
//!   factory whether to build, subscribes synchronously inside
//!   the callback, and spawns one tokio drain task per agent
//!   instance.
//! * [`AgentRunnerHandle`]: cheaply-cloneable handle returned
//!   from [`AgentRunner::install`]. Holds the spawned drain
//!   tasks alive for the server lifetime; exposes per-
//!   `(agent, broadcast, track)` counters to tests and to
//!   future admin / metrics consumers.
//!
//! # Lifecycle
//!
//! Per the plan, "agents spawn on `BroadcastStarted`, stop on
//! `BroadcastStopped`. Failures in an agent do NOT propagate
//! to the broadcast; a panic in an agent thread is caught and
//! logged."
//!
//! In this crate that maps to:
//!
//! * **Spawn on `BroadcastStarted`**: `on_entry_created` fires
//!   on the first `get_or_create` for a new `(broadcast, track)`
//!   pair. The runner's callback subscribes to the new
//!   broadcaster and spawns a drain task per agent the
//!   factories opt in.
//! * **Stop on `BroadcastStopped`**: the drain loop terminates
//!   naturally when every producer-side clone of the
//!   broadcaster has been dropped (i.e. after
//!   `lvqr_ingest::bridge`'s `on_unpublish` calls
//!   `registry.remove` and the bridge's `streams: DashMap`
//!   entry drops). At that point the
//!   [`lvqr_fragment::BroadcasterStream`] sees `Closed`,
//!   `next_fragment()` returns `None`, the drain loop exits,
//!   and the agent's [`Agent::on_stop`] runs. There is no
//!   separate `on_entry_removed` wiring because the natural
//!   teardown path already covers it; adding one would race
//!   with the drain loop already in flight.
//! * **Panic isolation**: every `on_start` / `on_fragment` /
//!   `on_stop` call is wrapped in
//!   `std::panic::catch_unwind(std::panic::AssertUnwindSafe(..))`.
//!   A panic in `on_start` skips the drain loop entirely
//!   (no fragments processed, no `on_stop`). A panic in
//!   `on_fragment` is logged + counted on
//!   [`AgentStats::panics`] and the loop continues with the
//!   next fragment -- one bad frame must not kill the agent.
//!   A panic in `on_stop` is logged + counted but otherwise
//!   absorbed.
//!
//! # Where this crate fits in the consumer family
//!
//! Pattern-matches the four existing
//! `FragmentBroadcasterRegistry` consumers:
//!
//! | Crate | Wires | Purpose |
//! |-------|-------|---------|
//! | `lvqr_cli::hls::BroadcasterHlsBridge` | `on_entry_created` | LL-HLS playlist composition |
//! | `lvqr_cli::archive::BroadcasterArchiveIndexer` | `on_entry_created` (+ drain-end C2PA finalize) | DVR archive index + on-disk segments |
//! | `lvqr_wasm::install_wasm_filter_bridge` | `on_entry_created` | Per-fragment WASM filter tap |
//! | `lvqr_cli::cluster_claim::install_cluster_claim_bridge` | `on_entry_created` | Renew cluster broadcast claim |
//! | **`lvqr_agent::AgentRunner`** (new) | `on_entry_created` | Per-broadcast user-defined agents |
//!
//! No new abstractions invented: the trait surface is a
//! one-method generalisation of what every existing consumer
//! already does inline (the `WasmFilterBridgeHandle`'s
//! per-`(broadcast, track)` `FilterStats` are the closest
//! sibling), and the install / drain pattern is byte-for-byte
//! the same as the four reference call sites.
//!
//! # Anti-scope (session 97 A)
//!
//! * **No CLI wiring.** Session 98 (whisper captions) will
//!   thread an `AgentRunner` through `lvqr_cli::start` once
//!   there is a concrete agent to register; this session
//!   leaves the CLI untouched.
//! * **No concrete agent.** No whisper, no symphonia. The
//!   first concrete `Agent` impl is session 98 B.
//! * **No `on_entry_removed` wiring.** The drain loop's
//!   natural termination IS the broadcast-stop signal. A
//!   second teardown channel would race the drain loop and
//!   double-fire `on_stop`.
//! * **No multi-agent conversation.** Per the
//!   `tracking/TIER_4_PLAN.md` section 4.5 anti-scope: the
//!   `Agent` trait is a `fn(&Fragment) -> ()` stream
//!   processor, not a goal-directed agent in the LLM sense.

mod agent;
mod factory;
mod runner;

pub use agent::{Agent, AgentContext};
pub use factory::AgentFactory;
pub use runner::{AgentRunner, AgentRunnerHandle, AgentStats};
