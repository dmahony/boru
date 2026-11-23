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
    use std::sync::mpsc;

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
        let cancel = RequestCancel::new();
        let worker_token = cancel.clone();
        let observed_token = worker_token.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let task = tokio::spawn(run_cancellable_blocking(cancel, move || {
            started_tx
                .send(())
                .map_err(|e| AppError::Internal(e.to_string()))?;
            let _ = release_rx.recv();
            if worker_token.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            Ok(())
        }));
        started_rx.recv().expect("worker started");
        task.abort();
        // The future's guard is dropped when abort is observed by the runtime.
        tokio::task::yield_now().await;
        release_tx.send(()).expect("worker waiting");
        let result = task.await;
        assert!(result.is_err());
        assert!(observed_token.is_cancelled());
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
