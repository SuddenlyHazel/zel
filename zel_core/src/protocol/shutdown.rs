use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::{timeout, Duration};

/// Tracks active asynchronous tasks and coordinates shutdown
///
/// TaskTracker provides a way to monitor the number of active operations
/// and wait for all of them to complete during graceful shutdown.
#[derive(Clone)]
pub struct TaskTracker {
    active_tasks: Arc<AtomicUsize>,
    completion_notify: Arc<Notify>,
}

impl TaskTracker {
    /// Create a new task tracker
    pub fn new() -> Self {
        Self {
            active_tasks: Arc::new(AtomicUsize::new(0)),
            completion_notify: Arc::new(Notify::new()),
        }
    }

    /// Increment task counter and return a guard
    ///
    /// The guard automatically decrements the counter when dropped,
    /// ensuring accurate tracking even if tasks panic.
    pub fn increment(&self) -> TaskGuard {
        self.active_tasks.fetch_add(1, Ordering::Relaxed);
        TaskGuard {
            tracker: self.clone(),
        }
    }

    /// Get current count of active tasks
    pub fn count(&self) -> usize {
        self.active_tasks.load(Ordering::Relaxed)
    }

    /// Wait for all active tasks to complete or timeout
    ///
    /// Returns `Ok(())` if all tasks completed within the timeout,
    /// `Err(())` if the timeout was reached with tasks still active.
    pub async fn wait_for_completion(&self, max_wait: Duration) -> Result<(), ()> {
        if self.count() == 0 {
            return Ok(());
        }

        timeout(max_wait, async {
            loop {
                if self.count() == 0 {
                    break;
                }
                self.completion_notify.notified().await;
            }
        })
        .await
        .map_err(|_| ())
    }
}

impl Default for TaskTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Guard that tracks the lifetime of a task
///
/// Automatically decrements the task counter when dropped,
/// ensuring accurate tracking even if the task panics.
pub struct TaskGuard {
    tracker: TaskTracker,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        let prev = self.tracker.active_tasks.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 {
            // We were the last task, notify all waiters
            self.tracker.completion_notify.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_task_tracker_basic() {
        let tracker = TaskTracker::new();
        assert_eq!(tracker.count(), 0);

        let guard1 = tracker.increment();
        assert_eq!(tracker.count(), 1);

        let guard2 = tracker.increment();
        assert_eq!(tracker.count(), 2);

        drop(guard1);
        assert_eq!(tracker.count(), 1);

        drop(guard2);
        assert_eq!(tracker.count(), 0);
    }

    #[tokio::test]
    async fn test_wait_for_completion_success() {
        let tracker = TaskTracker::new();

        let guard = tracker.increment();

        // Spawn task that drops guard after 100ms
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(guard);
        });

        // Should complete before timeout
        assert!(tracker
            .wait_for_completion(Duration::from_secs(1))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_wait_for_completion_timeout() {
        let tracker = TaskTracker::new();

        let _guard = tracker.increment();

        // Should timeout (guard is never dropped)
        assert!(tracker
            .wait_for_completion(Duration::from_millis(50))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_wait_for_completion_already_zero() {
        let tracker = TaskTracker::new();

        // Should return immediately when no tasks are active
        assert!(tracker
            .wait_for_completion(Duration::from_secs(1))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_multiple_tasks_complete() {
        let tracker = TaskTracker::new();

        let mut handles = vec![];
        for i in 0..10 {
            let guard = tracker.increment();
            handles.push(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10 * i)).await;
                drop(guard);
            }));
        }

        assert_eq!(tracker.count(), 10);

        // Wait for all to complete
        assert!(tracker
            .wait_for_completion(Duration::from_secs(2))
            .await
            .is_ok());
        assert_eq!(tracker.count(), 0);
    }
}
