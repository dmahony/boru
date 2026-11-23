//! Cancellation helper for message-page commands.
//!
//! The command should create one token per React Query invocation and cancel it
//! when the query's `AbortSignal` fires (including unmount); pass a clone into
//! `run_cancellable_blocking`. SQLite storage here owns a locked connection per
//! call rather than a pool; connection lifetime is scoped and released by RAII.
use crate::bridge::AppError;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone)]
pub struct RequestCancel {
    cancelled: Arc<AtomicBool>,
}

struct RequestGuard(Arc<AtomicBool>);

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl RequestCancel {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Default for RequestCancel {
    fn default() -> Self {
        Self::new()
    }
}

/// Run one blocking operation, observing cancellation around its work.
pub async fn run_cancellable_blocking<T, F>(cancel: RequestCancel, work: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    let _guard = RequestGuard(Arc::clone(&cancel.cancelled));
    let task = tauri::async_runtime::spawn_blocking(move || {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let result = work();
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        result
    });
    match task.await {
        Ok(result) => result,
        Err(error) => Err(AppError::Internal(format!("blocking task failed: {error}"))),
    }
    // guard drop marks cancellation when the caller's future completes or is dropped.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_before_work_skips_closure() {
        let cancel = RequestCancel::new();
        cancel.cancel();
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let result = run_cancellable_blocking(cancel, move || {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn dropped_caller_guard_cancels_mid_flight() {
        // The caller future is aborted while the blocking work is in flight; the
        // dropped request guard must flip the shared token the worker observes.
        //
        // The handshakes are bounded atomic/yield loops, never blocking channel
        // receives: a `recv()` that waits on the test task can deadlock the
        // single-threaded test runtime against the blocking pool thread.
        let cancel = RequestCancel::new();
        let worker_token = cancel.clone();
        let observed_token = cancel.clone();
        let started = Arc::new(AtomicBool::new(false));
        let started_by_worker = Arc::clone(&started);

        let task = tokio::spawn(run_cancellable_blocking(cancel, move || {
            started_by_worker.store(true, Ordering::SeqCst);
            for _ in 0..5_000 {
                if worker_token.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Ok(())
        }));

        for _ in 0..10_000 {
            if started.load(Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            started.load(Ordering::SeqCst),
            "blocking work never started before the caller was aborted"
        );

        task.abort();
        let result = task.await;
        assert!(result.is_err(), "aborted caller future must not yield a value");
        assert!(
            observed_token.is_cancelled(),
            "dropping the caller future must cancel the request"
        );
    }

    #[tokio::test]
    async fn happy_path_returns_value() {
        let result = run_cancellable_blocking(RequestCancel::new(), || Ok(42)).await;
        assert!(matches!(result, Ok(42)));
    }

    #[tokio::test]
    async fn panic_maps_to_internal() {
        let result =
            run_cancellable_blocking::<(), _>(RequestCancel::new(), || panic!("test panic")).await;
        assert!(matches!(result, Err(AppError::Internal(_))));
    }
}
