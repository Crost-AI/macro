#![deny(missing_docs)]
//! The impure twin of [`sync_machine`]: the same document-sync component
//! implemented as an async actor with ports, for side-by-side comparison.
//!
//! Same vocabulary (this crate reuses `sync_machine::model` and its
//! [`Replica`](sync_machine::replica::Replica) trait), same behavioral scope:
//! load-then-serve, ack-after-durable, debounced snapshot compaction, idle
//! eviction, persist retry, presence, blame, lifecycle events.
//!
//! The structural differences to compare:
//!
//! - **IO is awaited inline.** `store.append_ops(...).await` sits in the
//!   middle of update handling, so ack-after-durable is just program order —
//!   no tokens, no pending-ack queue, no completion inputs. The price is that
//!   the *entire document* stalls for the duration of every store call
//!   (frames wait in the mailbox), where the pure machine keeps serving
//!   presence/reads while a persist is in flight.
//! - **Time is real.** Debounce and idle eviction are `tokio::time` deadlines
//!   in a `select!`; tests need `start_paused` + `advance` instead of feeding
//!   a `TimerFired` value.
//! - **Effects are invisible.** Sends, blame, lifecycle land via port calls
//!   as they happen; there is no effect list to assert against, so tests
//!   observe mocks (shared logs) instead of data.
//! - **Eviction is process death.** The actor breaks its loop; the supervisor
//!   discovers the closed channel on next use and respawns.

pub mod domain;
