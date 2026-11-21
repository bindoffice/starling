use super::*;
use std::future::Future;

pub(in crate::server) enum DispatchWait<T> {
    Completed { output: T, client_closed: bool },
    Cancelled,
}

pub(in crate::server) async fn wait_for_dispatch<F>(
    dispatch: F,
    control: Option<&ClientSocketControl>,
    client_abort: &ClientAbortState,
) -> DispatchWait<F::Output>
where
    F: Future,
{
    let mut dispatch = Box::pin(dispatch);
    let mut client_closed = false;
    loop {
        if client_closed {
            return DispatchWait::Completed {
                output: dispatch.as_mut().await,
                client_closed,
            };
        }
        let Some(control) = control else {
            return DispatchWait::Completed {
                output: dispatch.as_mut().await,
                client_closed,
            };
        };
        tokio::select! {
            output = &mut dispatch => {
                if control.close_detected()
                    && client_abort.upstream_active()
                    && !client_abort.preserve()
                {
                    return DispatchWait::Cancelled;
                }
                return DispatchWait::Completed { output, client_closed };
            }
            _ = wait_for_client_close(control) => {
                if client_abort.preserve() {
                    client_closed = true;
                } else if client_abort.upstream_active() {
                    return DispatchWait::Cancelled;
                }
            }
        }
    }
}

pub(in crate::server) async fn wait_for_client_close(control: &ClientSocketControl) {
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if control.close_detected() {
            return;
        }
    }
}

pub(in crate::server) async fn wait_for_client_reset(control: &ClientSocketControl) {
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if control.reset_detected() {
            return;
        }
    }
}
