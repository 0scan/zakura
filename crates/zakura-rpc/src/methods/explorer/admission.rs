//! Admission control for explorer reads that resolve historical transparent prevouts.
//!
//! Address histories and block or transaction details decode every transaction they return
//! plus the parents of its transparent inputs. A handful of these for old addresses, whose
//! inputs spend large mining-pool payouts, can take tens of seconds of disk reads, so the
//! node only runs a few at a time and reports the rest as busy.

use std::{
    future::Future,
    sync::{Arc, LazyLock},
    time::Duration,
};

use tokio::sync::Semaphore;

use crate::server::jsonrpc::{ErrorCode, ErrorObject, RpcResult};

/// How long a heavy read waits for a free slot before the node reports that it is busy.
const QUEUE_TIMEOUT: Duration = Duration::from_secs(5);

/// The JSON-RPC "server is busy" error code, matching the code jsonrpsee used.
pub(super) const SERVER_BUSY: i32 = -32009;

/// Slots for `getaddresstransactions`, the read crawlers request most.
pub(super) static ADDRESS_HISTORY_READS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(2)));

/// Slots for `getblockdetails` and `gettransactiondetails`, kept separate so address crawls
/// cannot starve the indexer's own block reads.
pub(super) static DETAIL_READS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(4)));

/// Runs `read` once one of `slots` is free, or fails with [`SERVER_BUSY`].
///
/// The read runs in its own task that owns the slot. When the client disconnects or times out,
/// the state reads it already started keep running, so the slot stays taken until they finish
/// and the limit reflects the work the node is actually doing.
pub(super) async fn run_heavy_read<T, F>(slots: &LazyLock<Arc<Semaphore>>, read: F) -> RpcResult<T>
where
    F: Future<Output = RpcResult<T>> + Send + 'static,
    T: Send + 'static,
{
    let slot = tokio::time::timeout(QUEUE_TIMEOUT, Arc::clone(slots).acquire_owned())
        .await
        .map_err(|_| {
            ErrorObject::new(
                ErrorCode::ServerError(SERVER_BUSY),
                "explorer is busy with other heavy reads, retry shortly",
            )
        })?
        .expect("explorer read semaphores are never closed");

    tokio::spawn(async move {
        let _slot = slot;
        read.await
    })
    .await
    .map_err(|error| {
        ErrorObject::new(
            ErrorCode::InternalError,
            format!("explorer read task failed: {error}"),
        )
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn busy_slots_fail_after_the_queue_timeout() {
        static SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));
        let held = Arc::clone(&SLOTS).acquire_owned().await.unwrap();

        let error = run_heavy_read(&SLOTS, async { Ok(()) })
            .await
            .expect_err("no slot frees up within the queue timeout");
        assert_eq!(error.code(), SERVER_BUSY);

        drop(held);
        run_heavy_read(&SLOTS, async { Ok(()) })
            .await
            .expect("a freed slot admits the next read");
    }

    #[tokio::test]
    async fn an_abandoned_read_keeps_its_slot_until_it_finishes() {
        static SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));
        let (finish, finished) = tokio::sync::oneshot::channel::<()>();

        let caller = tokio::spawn(run_heavy_read(&SLOTS, async move {
            let _ = finished.await;
            Ok(())
        }));
        while SLOTS.available_permits() != 0 {
            tokio::task::yield_now().await;
        }

        // Drop the caller, as the server does when a client disconnects.
        caller.abort();
        let _ = caller.await;
        assert_eq!(SLOTS.available_permits(), 0, "the read still owns its slot");

        finish.send(()).unwrap();
        while SLOTS.available_permits() != 1 {
            tokio::task::yield_now().await;
        }
    }
}
