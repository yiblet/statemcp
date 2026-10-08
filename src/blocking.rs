//! Bounded async entry into the synchronous application controllers.
use state_core::{Controller, Error as CoreError, ErrorCode};
use std::{fmt, sync::Arc};
use tokio::{sync::Semaphore, task::JoinError};

/// Shared application dependency for transports that execute blocking use cases.
/// Clones share admission; a canceled caller cannot release a running operation's slot.
#[derive(Clone)]
pub struct BlockingExecutor {
    controller: Controller,
    slots: Arc<Semaphore>,
}

/// Application failure or failure of the blocking execution task itself.
#[derive(Debug)]
pub enum BlockingError {
    /// Includes admission exhaustion (`BUSY`) and errors returned by a controller.
    Application(CoreError),
    /// A task failed before it could return an application result.
    Task(JoinError),
}
impl fmt::Display for BlockingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Application(error) => error.fmt(formatter),
            Self::Task(error) => write!(formatter, "blocking application task failed: {error}"),
        }
    }
}
impl std::error::Error for BlockingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Application(error) => Some(error),
            Self::Task(error) => Some(error),
        }
    }
}
impl BlockingExecutor {
    /// Construct a shared executor with sixteen available operation slots.
    /// Construction performs no storage or runtime I/O.
    pub fn new(controller: Controller) -> Self {
        let capacity = 16.min(Semaphore::MAX_PERMITS);
        // SAFETY: capacity is at most Semaphore::MAX_PERMITS, the constructor's bound.
        let slots = Semaphore::new(capacity);
        Self {
            controller,
            slots: Arc::new(slots),
        }
    }

    /// Configure a nonzero admission limit within Tokio's supported permit range.
    /// Returns INVALID_ARGUMENT for an invalid limit; performs no application I/O.
    pub fn with_capacity(controller: Controller, capacity: usize) -> Result<Self, CoreError> {
        if capacity == 0 || capacity > Semaphore::MAX_PERMITS {
            return Err(CoreError::new(
                ErrorCode::InvalidArgument,
                "invalid blocking admission capacity",
            ));
        }
        // SAFETY: the check above establishes Tokio's maximum permit bound.
        let slots = Semaphore::new(capacity);
        Ok(Self {
            controller,
            slots: Arc::new(slots),
        })
    }

    /// Admit a use case immediately, then run its complete transaction on a blocking thread.
    /// Returns BUSY without scheduling work if all slots are occupied. Controller errors
    /// remain typed; panics/task failures are reported separately. Cancellation stops waiting
    /// but does not cancel already running work or release its slot before completion.
    pub async fn run<T: Send + 'static>(
        &self,
        action: impl FnOnce(Controller) -> state_core::Result<T> + Send + 'static,
    ) -> Result<T, BlockingError> {
        let permit = self.slots.clone().try_acquire_owned().map_err(|_| {
            BlockingError::Application(CoreError::new(
                ErrorCode::Busy,
                "Too many active invocations; retry later",
            ))
        })?;
        let controller = self.controller.clone();
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _span = span.enter();
            action(controller)
        })
        .await
        .map_err(BlockingError::Task)?
        .map_err(BlockingError::Application)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;
    use std::sync::mpsc;
    use tokio::sync::oneshot;

    #[tokio::test(flavor = "current_thread")]
    async fn clones_share_admission_and_cancellation_keeps_running_work_admitted()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new();
        let executor = BlockingExecutor::with_capacity(fixture.state_async().await?, 1)?;
        let worker = executor.clone();
        let (entered, started) = oneshot::channel();
        let (release, blocked) = mpsc::channel();
        let pending = tokio::spawn(async move {
            worker
                .run(move |_| {
                    entered.send(()).map_err(|_| {
                        CoreError::new(ErrorCode::WorkerFailed, "test receiver dropped")
                    })?;
                    blocked.recv().map_err(|_| {
                        CoreError::new(ErrorCode::WorkerFailed, "test sender dropped")
                    })?;
                    Ok(())
                })
                .await
        });
        started.await?;
        pending.abort();
        assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
        let refusal = executor.clone().run(|_| Ok(())).await;
        // Unblock even if the assertion below fails, so no blocking worker is stranded.
        release.send(())?;
        assert!(
            matches!(refusal, Err(BlockingError::Application(error)) if error.code == ErrorCode::Busy)
        );
        // Await actual permit release rather than relying on scheduling or a sleep.
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            executor.slots.clone().acquire_owned(),
        )
        .await??;
        drop(permit);
        executor.run(|_| Ok(())).await?;
        Ok(())
    }

    #[tokio::test]
    async fn application_errors_remain_typed_and_invalid_capacity_is_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        let controller = Fixture::new().state_async().await?;
        assert!(
            matches!(BlockingExecutor::with_capacity(controller.clone(), 0), Err(error) if error.code == ErrorCode::InvalidArgument)
        );
        assert!(
            matches!(BlockingExecutor::with_capacity(controller.clone(), usize::MAX), Err(error) if error.code == ErrorCode::InvalidArgument)
        );
        let result = BlockingExecutor::new(controller)
            .run::<()>(|_| Err(CoreError::new(ErrorCode::Conflict, "controlled failure")))
            .await;
        assert!(
            matches!(result, Err(BlockingError::Application(error)) if error.code == ErrorCode::Conflict)
        );
        Ok(())
    }
}
