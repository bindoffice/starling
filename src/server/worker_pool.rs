use std::net::{TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::thread;

#[cfg(unix)]
use std::os::unix::net::UnixListener as StdUnixListener;

use super::*;

pub(in crate::server) struct WorkerPool {
    exits: mpsc::UnboundedReceiver<()>,
    handles: Vec<thread::JoinHandle<()>>,
    runtimes: Vec<tokio::runtime::Runtime>,
    logger: Arc<Logger>,
    unix_socket_paths: UnixSocketCleanup,
}

impl WorkerPool {
    pub(in crate::server) async fn start(
        groups: Vec<ListenerGroup>,
        stream_routes: Vec<StreamRoute>,
        mail_routes: Vec<MailRoute>,
        state: SharedState,
        shutdown: watch::Receiver<ShutdownMode>,
        shutdown_tx: watch::Sender<ShutdownMode>,
        worker_layout: Vec<usize>,
        blocking_thread_limit: usize,
    ) -> io::Result<Self> {
        let worker_layout = if worker_layout.is_empty() {
            vec![1]
        } else {
            worker_layout
        };
        let workers = worker_layout.len();
        let blocking_threads = (blocking_thread_limit / workers).max(1);
        let mut specs: Vec<WorkerSpec> = (0..workers).map(WorkerSpec::new).collect();
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut unix_socket_paths = UnixSocketCleanup::default();

        for group in groups {
            if group.tcp {
                match group.address.clone() {
                    ListenAddress::Inet(address) => {
                        let listener =
                            bind_tcp_listener(address, &group.socket_options).map_err(|err| {
                                io::Error::new(
                                    err.kind(),
                                    format!("bind() to {address} failed: {err}"),
                                )
                            })?;
                        let listeners = clone_tcp_listener(listener.into_std()?, workers)?;
                        for (spec, listener) in specs.iter_mut().zip(listeners) {
                            spec.http_tcp.push((listener, group.clone()));
                        }
                    }
                    ListenAddress::Unix(path) => {
                        #[cfg(unix)]
                        {
                            let listener = bind_unix_listener(&path, &group.socket_options)
                                .map_err(|err| {
                                    io::Error::new(
                                        err.kind(),
                                        format!("bind() to unix:{} failed: {err}", path.display()),
                                    )
                                })?;
                            unix_socket_paths.paths.push(path);
                            let listeners = clone_unix_listener(listener.into_std()?, workers)?;
                            for (spec, listener) in specs.iter_mut().zip(listeners) {
                                spec.http_unix.push((listener, group.clone()));
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            let _ = path;
                            return Err(io::Error::new(
                                io::ErrorKind::Unsupported,
                                "UNIX domain listen sockets are unsupported on this platform",
                            ));
                        }
                    }
                }
            }
            if group.quic {
                let ListenAddress::Inet(address) = group.address.clone() else {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        format!("QUIC bind() to {} is unsupported", group.address),
                    ));
                };
                let socket = bind_udp_socket(address, &group.socket_options).map_err(|err| {
                    io::Error::new(err.kind(), format!("bind() to udp:{address} failed: {err}"))
                })?;
                specs[0].http_quic.push((socket.into_std()?, group));
            }
        }

        for route in stream_routes {
            if route.udp {
                let ListenAddress::Inet(address) = route.address.clone() else {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        format!("UDP bind() to {} is unsupported", route.address),
                    ));
                };
                let listener = bind_udp_socket(address, &route.socket_options).map_err(|err| {
                    io::Error::new(err.kind(), format!("bind() to udp:{address} failed: {err}"))
                })?;
                // UDP sessions own per-peer upstream state, so keep each socket
                // on one worker instead of distributing datagrams across maps.
                specs[0].stream_udp.push((listener.into_std()?, route));
                continue;
            }
            match route.address.clone() {
                ListenAddress::Inet(address) => {
                    let listener =
                        bind_tcp_listener(address, &route.socket_options).map_err(|err| {
                            io::Error::new(err.kind(), format!("bind() to {address} failed: {err}"))
                        })?;
                    let listeners = clone_tcp_listener(listener.into_std()?, workers)?;
                    for (spec, listener) in specs.iter_mut().zip(listeners) {
                        spec.stream_tcp.push((listener, route.clone()));
                    }
                }
                ListenAddress::Unix(path) => {
                    #[cfg(unix)]
                    {
                        let listener =
                            bind_unix_listener(&path, &route.socket_options).map_err(|err| {
                                io::Error::new(
                                    err.kind(),
                                    format!("bind() to unix:{} failed: {err}", path.display()),
                                )
                            })?;
                        unix_socket_paths.paths.push(path);
                        let listeners = clone_unix_listener(listener.into_std()?, workers)?;
                        for (spec, listener) in specs.iter_mut().zip(listeners) {
                            spec.stream_unix.push((listener, route.clone()));
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = path;
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "UNIX domain stream listen sockets are unsupported on this platform",
                        ));
                    }
                }
            }
        }

        for route in mail_routes {
            match route.address.clone() {
                ListenAddress::Inet(address) => {
                    let listener =
                        bind_tcp_listener(address, &route.socket_options).map_err(|err| {
                            io::Error::new(err.kind(), format!("bind() to {address} failed: {err}"))
                        })?;
                    let listeners = clone_tcp_listener(listener.into_std()?, workers)?;
                    for (spec, listener) in specs.iter_mut().zip(listeners) {
                        spec.mail_tcp.push((listener, route.clone()));
                    }
                }
                ListenAddress::Unix(path) => {
                    #[cfg(unix)]
                    {
                        let listener =
                            bind_unix_listener(&path, &route.socket_options).map_err(|err| {
                                io::Error::new(
                                    err.kind(),
                                    format!("bind() to unix:{} failed: {err}", path.display()),
                                )
                            })?;
                        unix_socket_paths.paths.push(path);
                        let listeners = clone_unix_listener(listener.into_std()?, workers)?;
                        for (spec, listener) in specs.iter_mut().zip(listeners) {
                            spec.mail_unix.push((listener, route.clone()));
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = path;
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "UNIX domain mail listen sockets are unsupported on this platform",
                        ));
                    }
                }
            }
        }

        let mut prepared = Vec::with_capacity(workers);
        for (spec, worker_threads) in specs.into_iter().zip(worker_layout) {
            let runtime = match build_worker_runtime(worker_threads, blocking_threads) {
                Ok(runtime) => runtime,
                Err(err) => {
                    let runtimes = prepared
                        .into_iter()
                        .map(|(runtime, _, _)| runtime)
                        .collect();
                    shutdown_runtimes_off_context(runtimes).await;
                    return Err(err);
                }
            };
            let worker = {
                let _guard = runtime.enter();
                match spec.prepare() {
                    Ok(worker) => worker,
                    Err(err) => {
                        let mut runtimes = prepared
                            .into_iter()
                            .map(|(runtime, _, _)| runtime)
                            .collect::<Vec<_>>();
                        runtimes.push(runtime);
                        shutdown_runtimes_off_context(runtimes).await;
                        return Err(err);
                    }
                }
            };
            prepared.push((runtime, worker, worker_threads));
        }

        let (exit_tx, exits) = mpsc::unbounded_channel();
        let mut handles = Vec::with_capacity(workers);
        let mut runtimes = Vec::with_capacity(workers);
        for (runtime, worker, worker_threads) in prepared {
            let mut state = state.clone();
            // Match nginx's per-worker keepalive cache even though workers share
            // one process and one backing pool.
            state.upstream_keepalive_worker = worker.id;
            let shutdown = shutdown.clone();
            let exit_tx = exit_tx.clone();
            let worker_id = worker.id;
            if worker_threads > 1 {
                runtime.handle().spawn(async move {
                    let mut notice = ExitNotice::new(exit_tx);
                    worker_main(worker, state, shutdown).await;
                    notice.notify();
                });
                runtimes.push(runtime);
                continue;
            }
            let runtime_slot = Arc::new(std::sync::Mutex::new(Some(runtime)));
            let worker_runtime = runtime_slot.clone();
            let spawn = thread::Builder::new()
                .name(format!("starling-worker-{worker_id}"))
                .stack_size(crate::runtime::WORKER_THREAD_STACK_SIZE)
                .spawn(move || {
                    let runtime = worker_runtime
                        .lock()
                        .expect("worker runtime slot poisoned")
                        .take()
                        .expect("worker runtime was already taken");
                    let mut notice = ExitNotice::new(exit_tx);
                    runtime.block_on(worker_main(worker, state, shutdown));
                    notice.notify();
                });
            match spawn {
                Ok(handle) => {
                    handles.push(handle);
                    drop(runtime_slot);
                }
                Err(err) => {
                    let failed_runtime = runtime_slot
                        .lock()
                        .expect("worker runtime slot poisoned")
                        .take();
                    let _ = shutdown_tx.send(ShutdownMode::Fast);
                    for handle in handles {
                        let _ = handle.join();
                    }
                    if let Some(runtime) = failed_runtime {
                        runtimes.push(runtime);
                    }
                    shutdown_runtimes_off_context(runtimes).await;
                    return Err(io::Error::other(format!(
                        "failed to create worker thread: {err}"
                    )));
                }
            }
        }

        Ok(Self {
            exits,
            handles,
            runtimes,
            logger: state.logger.clone(),
            unix_socket_paths,
        })
    }

    pub(in crate::server) async fn wait(mut self) {
        let worker_count = self.handles.len() + self.runtimes.len();
        for _ in 0..worker_count {
            if self.exits.recv().await.is_none() {
                break;
            }
        }
        for (worker_id, handle) in self.handles.drain(..).enumerate() {
            if handle.join().is_err() {
                self.logger
                    .log(LogLevel::Error, format!("worker {worker_id} thread failed"));
            }
        }
        let runtimes = std::mem::take(&mut self.runtimes);
        shutdown_runtimes_off_context(runtimes).await;
        drop(self.unix_socket_paths);
    }
}

async fn shutdown_runtimes_off_context(runtimes: Vec<tokio::runtime::Runtime>) {
    if runtimes.is_empty() {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || {
        for runtime in runtimes {
            runtime.shutdown_timeout(Duration::from_millis(100));
        }
    })
    .await;
}

fn build_worker_runtime(
    threads: usize,
    blocking_threads: usize,
) -> io::Result<tokio::runtime::Runtime> {
    let mut builder = if threads > 1 {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.worker_threads(threads);
        builder
    } else {
        tokio::runtime::Builder::new_current_thread()
    };
    builder
        .enable_all()
        .max_blocking_threads(blocking_threads.max(1))
        .thread_name("starling-worker")
        .thread_stack_size(crate::runtime::WORKER_THREAD_STACK_SIZE)
        .build()
        .map_err(|err| io::Error::other(format!("failed to create worker async runtime: {err}")))
}

struct WorkerSpec {
    id: usize,
    http_tcp: Vec<(StdTcpListener, ListenerGroup)>,
    http_quic: Vec<(StdUdpSocket, ListenerGroup)>,
    stream_tcp: Vec<(StdTcpListener, StreamRoute)>,
    stream_udp: Vec<(StdUdpSocket, StreamRoute)>,
    mail_tcp: Vec<(StdTcpListener, MailRoute)>,
    #[cfg(unix)]
    http_unix: Vec<(StdUnixListener, ListenerGroup)>,
    #[cfg(unix)]
    stream_unix: Vec<(StdUnixListener, StreamRoute)>,
    #[cfg(unix)]
    mail_unix: Vec<(StdUnixListener, MailRoute)>,
}

impl WorkerSpec {
    fn new(id: usize) -> Self {
        Self {
            id,
            http_tcp: Vec::new(),
            http_quic: Vec::new(),
            stream_tcp: Vec::new(),
            stream_udp: Vec::new(),
            mail_tcp: Vec::new(),
            #[cfg(unix)]
            http_unix: Vec::new(),
            #[cfg(unix)]
            stream_unix: Vec::new(),
            #[cfg(unix)]
            mail_unix: Vec::new(),
        }
    }

    fn prepare(self) -> io::Result<PreparedWorker> {
        let mut http_tcp = Vec::with_capacity(self.http_tcp.len());
        for (listener, group) in self.http_tcp {
            http_tcp.push((TcpListener::from_std(listener)?, group));
        }
        let http_quic = self.http_quic;
        let mut stream_tcp = Vec::with_capacity(self.stream_tcp.len());
        for (listener, route) in self.stream_tcp {
            stream_tcp.push((TcpListener::from_std(listener)?, route));
        }
        let mut stream_udp = Vec::with_capacity(self.stream_udp.len());
        for (socket, route) in self.stream_udp {
            stream_udp.push((UdpSocket::from_std(socket)?, route));
        }
        let mut mail_tcp = Vec::with_capacity(self.mail_tcp.len());
        for (listener, route) in self.mail_tcp {
            mail_tcp.push((TcpListener::from_std(listener)?, route));
        }
        #[cfg(unix)]
        let mut http_unix = Vec::with_capacity(self.http_unix.len());
        #[cfg(unix)]
        for (listener, group) in self.http_unix {
            http_unix.push((UnixListener::from_std(listener)?, group));
        }
        #[cfg(unix)]
        let mut stream_unix = Vec::with_capacity(self.stream_unix.len());
        #[cfg(unix)]
        for (listener, route) in self.stream_unix {
            stream_unix.push((UnixListener::from_std(listener)?, route));
        }
        #[cfg(unix)]
        let mut mail_unix = Vec::with_capacity(self.mail_unix.len());
        #[cfg(unix)]
        for (listener, route) in self.mail_unix {
            mail_unix.push((UnixListener::from_std(listener)?, route));
        }
        Ok(PreparedWorker {
            id: self.id,
            http_tcp,
            http_quic,
            stream_tcp,
            stream_udp,
            mail_tcp,
            #[cfg(unix)]
            http_unix,
            #[cfg(unix)]
            stream_unix,
            #[cfg(unix)]
            mail_unix,
        })
    }
}

struct PreparedWorker {
    id: usize,
    http_tcp: Vec<(TcpListener, ListenerGroup)>,
    http_quic: Vec<(StdUdpSocket, ListenerGroup)>,
    stream_tcp: Vec<(TcpListener, StreamRoute)>,
    stream_udp: Vec<(UdpSocket, StreamRoute)>,
    mail_tcp: Vec<(TcpListener, MailRoute)>,
    #[cfg(unix)]
    http_unix: Vec<(UnixListener, ListenerGroup)>,
    #[cfg(unix)]
    stream_unix: Vec<(UnixListener, StreamRoute)>,
    #[cfg(unix)]
    mail_unix: Vec<(UnixListener, MailRoute)>,
}

async fn worker_main(
    worker: PreparedWorker,
    state: SharedState,
    mut shutdown: watch::Receiver<ShutdownMode>,
) {
    let mut tasks = JoinSet::new();
    for (listener, group) in worker.http_tcp {
        tasks.spawn(accept_loop(
            listener,
            group,
            state.clone(),
            shutdown.clone(),
        ));
    }
    for (socket, group) in worker.http_quic {
        let tls_configs = state.tls_configs.read().await.get(&group.address).cloned();
        let Some(tls_configs) = tls_configs else {
            state.logger.log(
                LogLevel::Error,
                format!("no QUIC TLS configuration for listener {}", group.address),
            );
            continue;
        };
        let config = state.current_config();
        let server_config = match build_http3_server_config(&config, &group, tls_configs) {
            Ok(config) => config,
            Err(err) => {
                state.logger.log(
                    LogLevel::Error,
                    format!("cannot configure HTTP/3 for {}: {err}", group.address),
                );
                continue;
            }
        };
        let endpoint = match quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(server_config),
            socket,
            Arc::new(quinn::TokioRuntime),
        ) {
            Ok(endpoint) => endpoint,
            Err(err) => {
                state.logger.log(
                    LogLevel::Error,
                    format!("cannot start HTTP/3 listener {}: {err}", group.address),
                );
                continue;
            }
        };
        tasks.spawn(http3_accept_loop(
            endpoint,
            group,
            state.clone(),
            shutdown.clone(),
        ));
    }
    #[cfg(unix)]
    for (listener, group) in worker.http_unix {
        tasks.spawn(accept_unix_loop(
            listener,
            group,
            state.clone(),
            shutdown.clone(),
        ));
    }
    for (listener, route) in worker.stream_tcp {
        tasks.spawn(stream_accept_loop(
            listener,
            route,
            state.clone(),
            shutdown.clone(),
        ));
    }
    #[cfg(unix)]
    for (listener, route) in worker.stream_unix {
        tasks.spawn(stream_unix_accept_loop(
            listener,
            route,
            state.clone(),
            shutdown.clone(),
        ));
    }
    for (listener, route) in worker.stream_udp {
        tasks.spawn(stream_udp_loop(
            listener,
            route,
            state.clone(),
            shutdown.clone(),
        ));
    }
    for (listener, route) in worker.mail_tcp {
        tasks.spawn(mail_accept_loop(
            listener,
            route,
            state.clone(),
            shutdown.clone(),
        ));
    }
    #[cfg(unix)]
    for (listener, route) in worker.mail_unix {
        tasks.spawn(mail_unix_accept_loop(
            listener,
            route,
            state.clone(),
            shutdown.clone(),
        ));
    }

    while !tasks.is_empty() {
        if let Some(Err(err)) = tasks.join_next().await {
            state.logger.log(
                LogLevel::Error,
                format!("worker {} listener task failed: {err}", worker.id),
            );
        }
    }
    let _ = wait_for_shutdown(&mut shutdown).await;
}

struct ExitNotice {
    sender: Option<mpsc::UnboundedSender<()>>,
}

impl ExitNotice {
    fn new(sender: mpsc::UnboundedSender<()>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    fn notify(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(());
        }
    }
}

impl Drop for ExitNotice {
    fn drop(&mut self) {
        self.notify();
    }
}

fn clone_tcp_listener(listener: StdTcpListener, workers: usize) -> io::Result<Vec<StdTcpListener>> {
    let mut listeners = Vec::with_capacity(workers);
    for _ in 1..workers {
        listeners.push(listener.try_clone()?);
    }
    listeners.push(listener);
    Ok(listeners)
}

#[cfg(unix)]
fn clone_unix_listener(
    listener: StdUnixListener,
    workers: usize,
) -> io::Result<Vec<StdUnixListener>> {
    let mut listeners = Vec::with_capacity(workers);
    for _ in 1..workers {
        listeners.push(listener.try_clone()?);
    }
    listeners.push(listener);
    Ok(listeners)
}

#[derive(Default)]
pub(in crate::server) struct UnixSocketCleanup {
    paths: Vec<PathBuf>,
}

impl Drop for UnixSocketCleanup {
    fn drop(&mut self) {
        for path in self.paths.drain(..) {
            let _ = std::fs::remove_file(path);
        }
    }
}
