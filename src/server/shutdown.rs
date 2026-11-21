use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::server) enum TaskDrain {
    Completed,
    TimedOut,
}

pub(in crate::server) async fn drain_tasks(
    tasks: &mut JoinSet<()>,
    timeout: Option<Duration>,
    mut on_error: impl FnMut(tokio::task::JoinError),
) -> TaskDrain {
    async fn drain_all<F>(tasks: &mut JoinSet<()>, on_error: &mut F)
    where
        F: FnMut(tokio::task::JoinError),
    {
        while let Some(result) = tasks.join_next().await {
            if let Err(err) = result {
                on_error(err);
            }
        }
    }

    match timeout {
        Some(timeout) => {
            tokio::select! {
                _ = drain_all(tasks, &mut on_error) => TaskDrain::Completed,
                _ = tokio::time::sleep(timeout) => TaskDrain::TimedOut,
            }
        }
        None => {
            drain_all(tasks, &mut on_error).await;
            TaskDrain::Completed
        }
    }
}

pub(in crate::server) async fn finish_connection_tasks(
    mut connections: JoinSet<()>,
    mut shutdown_mode: ShutdownMode,
    mut shutdown: watch::Receiver<ShutdownMode>,
) {
    if shutdown_mode == ShutdownMode::Fast || *shutdown.borrow_and_update() == ShutdownMode::Fast {
        connections.shutdown().await;
        return;
    }

    loop {
        tokio::select! {
            joined = connections.join_next() => {
                if joined.is_none() {
                    return;
                }
            }
            changed = shutdown.changed() => {
                shutdown_mode = if changed.is_err() {
                    ShutdownMode::Fast
                } else {
                    *shutdown.borrow_and_update()
                };
                if shutdown_mode == ShutdownMode::Fast {
                    connections.shutdown().await;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use tokio::sync::oneshot;

    use super::*;

    struct AbortSignal(Option<oneshot::Sender<()>>);

    impl Drop for AbortSignal {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn drain_tasks_reports_timeout_and_can_resume_after_release() {
        let (release, released) = oneshot::channel();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            let _ = released.await;
        });

        assert_eq!(
            drain_tasks(&mut tasks, Some(Duration::from_millis(10)), |_| {}).await,
            TaskDrain::TimedOut
        );

        release.send(()).unwrap();
        assert_eq!(
            drain_tasks(&mut tasks, None, |_| {}).await,
            TaskDrain::Completed
        );
    }

    #[tokio::test]
    async fn fast_shutdown_cancels_connections_waiting_in_graceful_mode() {
        let (started, is_started) = oneshot::channel();
        let (aborted, is_aborted) = oneshot::channel();
        let mut connections = JoinSet::new();
        connections.spawn(async move {
            let _signal = AbortSignal(Some(aborted));
            let _ = started.send(());
            pending::<()>().await;
        });

        let (shutdown, receiver) = watch::channel(ShutdownMode::Graceful);
        let task = tokio::spawn(finish_connection_tasks(
            connections,
            ShutdownMode::Graceful,
            receiver,
        ));
        is_started.await.unwrap();
        assert!(!task.is_finished());

        shutdown.send(ShutdownMode::Fast).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("Fast shutdown should cancel tracked connections")
            .unwrap();
        is_aborted.await.expect("connection task should be aborted");
    }
}
