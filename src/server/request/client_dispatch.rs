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
    let mut close_seen = false;
    let mut preserve_seen = false;
    let mut client_closed = false;
    loop {
        let Some(control) = control else {
            return DispatchWait::Completed {
                output: dispatch.as_mut().await,
                client_closed: false,
            };
        };
        tokio::select! {
            output = &mut dispatch => {
                if control.close_detected()
                    && !client_abort.preserve()
                    && client_abort.upstream_active()
                {
                    client_abort.release_connection_limit();
                    return DispatchWait::Cancelled;
                }
                return DispatchWait::Completed { output, client_closed };
            }
            _ = wait_for_client_close(control), if !close_seen => {
                close_seen = true;
                if client_abort.preserve() {
                    preserve_seen = true;
                    client_closed = control.reset_detected();
                    continue;
                }
                client_abort.release_connection_limit();
                if client_abort.upstream_active() {
                    return DispatchWait::Cancelled;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(10)),
                if close_seen && !preserve_seen && !client_closed =>
            {
                if client_abort.preserve() {
                    preserve_seen = true;
                    client_closed = control.reset_detected();
                } else if client_abort.upstream_active() {
                    client_abort.release_connection_limit();
                    return DispatchWait::Cancelled;
                }
            }
        }
    }
}

pub(in crate::server) async fn wait_for_client_close(control: &ClientSocketControl) {
    loop {
        if control.close_detected() {
            return;
        }
        tokio::select! {
            _ = control.wait_for_close_event() => {}
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
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
