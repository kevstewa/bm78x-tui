//! The background tasks `main` runs: a scan and a session, each stopped by
//! a signal of its own, and each watched for ending on its own.

use std::fmt;
use std::future::{Future, pending};

use tokio::sync::oneshot;
use tokio::task::{JoinError, JoinHandle};

/// Which task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// The scan for meters.
    Scan,
    /// The session with a meter.
    Session,
}

impl fmt::Display for Which {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Which::Scan => "scan",
            Which::Session => "session",
        })
    }
}

/// Told to stop: resolves when the task should finish up.
pub type Stop = oneshot::Receiver<()>;

struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

/// At most one scan and one session.
#[derive(Default)]
pub struct Tasks {
    scan: Option<Running>,
    session: Option<Running>,
}

impl Tasks {
    /// Starts `task` as `which`, stopping the one before it first.
    pub async fn start<F>(&mut self, which: Which, task: impl FnOnce(Stop) -> F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.stop(which).await;
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(task(stopped));
        *self.slot(which) = Some(Running { stop, task });
    }

    /// Whether `which` is running.
    #[cfg(test)]
    pub fn running(&self, which: Which) -> bool {
        match which {
            Which::Scan => self.scan.is_some(),
            Which::Session => self.session.is_some(),
        }
    }

    /// Tells `which` to stop and waits until it has.
    pub async fn stop(&mut self, which: Which) {
        if let Some(Running { stop, task }) = self.slot(which).take() {
            let _ = stop.send(());
            let _ = task.await;
        }
    }

    /// Stops the scan, then the session.
    pub async fn stop_all(&mut self) {
        self.stop(Which::Scan).await;
        self.stop(Which::Session).await;
    }

    /// Waits for a task to end by itself, and says which and how. Never
    /// resolves while none is running.
    pub async fn ended(&mut self) -> (Which, Result<(), JoinError>) {
        async fn end(running: &mut Option<Running>) -> Result<(), JoinError> {
            match running {
                Some(running) => (&mut running.task).await,
                None => pending().await,
            }
        }
        let (which, result) = tokio::select! {
            result = end(&mut self.scan) => (Which::Scan, result),
            result = end(&mut self.session) => (Which::Session, result),
        };
        *self.slot(which) = None;
        (which, result)
    }

    fn slot(&mut self, which: Which) -> &mut Option<Running> {
        match which {
            Which::Scan => &mut self.scan,
            Which::Session => &mut self.session,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// A task that runs until stopped, then records that it finished.
    fn until_stopped(
        done: Arc<AtomicBool>,
    ) -> impl FnOnce(Stop) -> futures_util::future::BoxFuture<'static, ()> {
        move |stop| {
            Box::pin(async move {
                let _ = stop.await;
                done.store(true, Ordering::SeqCst);
            })
        }
    }

    #[tokio::test]
    async fn stopping_waits_for_the_task_to_finish() {
        let mut tasks = Tasks::default();
        let done = Arc::new(AtomicBool::new(false));
        tasks
            .start(Which::Session, until_stopped(done.clone()))
            .await;
        assert!(tasks.running(Which::Session));
        tasks.stop(Which::Session).await;
        assert!(done.load(Ordering::SeqCst));
        assert!(!tasks.running(Which::Session));
        // Stopping what is not running does nothing.
        tasks.stop(Which::Scan).await;
    }

    #[tokio::test]
    async fn starting_again_stops_the_old_task_first() {
        let mut tasks = Tasks::default();
        let first = Arc::new(AtomicBool::new(false));
        let second = Arc::new(AtomicBool::new(false));
        tasks.start(Which::Scan, until_stopped(first.clone())).await;
        tasks
            .start(Which::Scan, until_stopped(second.clone()))
            .await;
        assert!(first.load(Ordering::SeqCst));
        assert!(!second.load(Ordering::SeqCst));
        tasks.stop_all().await;
        assert!(second.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_task_that_ends_by_itself_is_reported() {
        let mut tasks = Tasks::default();
        tasks.start(Which::Scan, |_stop| async {}).await;
        let (which, result) = tokio::time::timeout(Duration::from_secs(5), tasks.ended())
            .await
            .unwrap();
        assert_eq!(which, Which::Scan);
        assert!(result.is_ok());
        // It can be started again.
        assert!(!tasks.running(Which::Scan));
    }

    #[tokio::test]
    async fn a_panicking_task_is_reported() {
        let mut tasks = Tasks::default();
        let done = Arc::new(AtomicBool::new(false));
        tasks.start(Which::Scan, until_stopped(done)).await;
        tasks
            .start(Which::Session, |_stop| async { panic!("test panic") })
            .await;
        let (which, result) = tokio::time::timeout(Duration::from_secs(5), tasks.ended())
            .await
            .unwrap();
        assert_eq!(which, Which::Session);
        assert!(result.unwrap_err().is_panic());
        assert!(tasks.running(Which::Scan));
    }

    #[tokio::test]
    async fn nothing_running_never_ends() {
        let mut tasks = Tasks::default();
        let waited = tokio::time::timeout(Duration::from_millis(50), tasks.ended()).await;
        assert!(waited.is_err());
    }
}
