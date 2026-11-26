use bytes::{Buf, Bytes};
use h3::client::SendRequest;
use h3_quinn::Connection as H3Connection;
use http::Request;
use quinn::crypto::rustls::QuicClientConfig;
use rustls::{ClientConfig, RootCertStore};
use std::{
    env,
    fs::File,
    io::BufReader,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::task::JoinSet;

type H3SendRequest = SendRequest<h3_quinn::OpenStreams, Bytes>;
type BenchConnection = (
    quinn::Connection,
    H3SendRequest,
    tokio::task::JoinHandle<()>,
);

#[derive(Debug)]
struct Config {
    address: SocketAddr,
    sni: String,
    certificate_authority: PathBuf,
    path: String,
    connections: usize,
    streams: usize,
    warmup: Duration,
    duration: Duration,
    rounds: usize,
}

impl Config {
    fn parse() -> Result<Self, String> {
        let mut config = Self {
            address: "127.0.0.1:443".parse().unwrap(),
            sni: "localhost".to_string(),
            certificate_authority: PathBuf::from("tests/fixtures/ca.crt"),
            path: "/".to_string(),
            connections: 16,
            streams: 16,
            warmup: Duration::from_secs(1),
            duration: Duration::from_secs(5),
            rounds: 1,
        };
        let mut arguments = env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let mut value = || {
                arguments
                    .next()
                    .ok_or_else(|| format!("missing value for {argument}"))
            };
            match argument.as_str() {
                "--addr" => {
                    config.address = value()?
                        .parse()
                        .map_err(|error| format!("invalid --addr: {error}"))?
                }
                "--sni" => config.sni = value()?,
                "--ca" => config.certificate_authority = PathBuf::from(value()?),
                "--path" => config.path = value()?,
                "--connections" => config.connections = parse_positive("--connections", &value()?)?,
                "--streams" => config.streams = parse_positive("--streams", &value()?)?,
                "--warmup" => config.warmup = parse_duration("--warmup", &value()?)?,
                "--duration" => config.duration = parse_duration("--duration", &value()?)?,
                "--rounds" => config.rounds = parse_positive("--rounds", &value()?)?,
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown option: {argument}")),
            }
        }
        if config.path.is_empty() || !config.path.starts_with('/') {
            return Err("--path must start with '/'".to_string());
        }
        Ok(config)
    }

    fn uri(&self) -> String {
        format!("https://{}{}", self.sni, self.path)
    }
}

fn parse_positive(option: &str, value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|error| format!("invalid {option}: {error}"))?;
    if parsed == 0 {
        return Err(format!("{option} must be greater than zero"));
    }
    Ok(parsed)
}

fn parse_duration(option: &str, value: &str) -> Result<Duration, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|error| format!("invalid {option}: {error}"))?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return Err(format!("{option} must be a positive number of seconds"));
    }
    Ok(Duration::from_secs_f64(parsed))
}

fn print_help() {
    println!(
        "h3load [options]\n\
         \n\
         HTTP/3 load generator for Starling/Nginx comparison.\n\
         \n\
         --addr ADDR        Server UDP address (default: 127.0.0.1:443)\n\
         --sni NAME         TLS server name and URI authority (default: localhost)\n\
         --ca PATH          CA certificate used to verify the server (default: tests/fixtures/ca.crt)\n\
         --path PATH        Request path (default: /)\n\
         --connections N    QUIC connections (default: 16)\n\
         --streams N        Concurrent requests per connection (default: 16)\n\
         --warmup SECONDS   Warmup duration (default: 1)\n\
         --duration SECONDS Measured duration (default: 5)\n\
         --rounds N          Measured rounds (default: 1)\n\
         -h, --help         Show this help"
    );
}

#[derive(Default)]
struct Phase {
    successes: u64,
    errors: u64,
    bytes: u64,
    latencies_us: Vec<u64>,
    first_error: Option<String>,
}

impl Phase {
    fn merge(&mut self, other: Self) {
        self.successes += other.successes;
        self.errors += other.errors;
        self.bytes += other.bytes;
        self.latencies_us.extend(other.latencies_us);
        if self.first_error.is_none() {
            self.first_error = other.first_error;
        }
    }

    fn percentile_ms(&mut self, percentile: f64) -> f64 {
        if self.latencies_us.is_empty() {
            return 0.0;
        }
        self.latencies_us.sort_unstable();
        let index = ((self.latencies_us.len() - 1) as f64 * percentile).ceil() as usize;
        self.latencies_us[index] as f64 / 1000.0
    }

    fn mean_ms(&self) -> f64 {
        if self.latencies_us.is_empty() {
            return 0.0;
        }
        self.latencies_us.iter().sum::<u64>() as f64 / self.latencies_us.len() as f64 / 1000.0
    }
}

async fn run_phase(
    send_request: H3SendRequest,
    uri: Arc<str>,
    streams: usize,
    duration: Duration,
) -> Phase {
    let deadline = Instant::now() + duration;
    let mut phase = Phase::default();
    let mut tasks = JoinSet::new();

    loop {
        while Instant::now() < deadline && tasks.len() < streams {
            let send_request = send_request.clone();
            let uri = uri.clone();
            tasks.spawn(async move { request_once(send_request, uri).await });
        }

        let Some(result) = tasks.join_next().await else {
            break;
        };
        match result {
            Ok(Ok((latency, bytes))) => {
                phase.successes += 1;
                phase.bytes += bytes;
                phase.latencies_us.push(latency);
            }
            Ok(Err(error)) => {
                phase.errors += 1;
                phase.first_error.get_or_insert(error);
            }
            Err(error) => {
                phase.errors += 1;
                phase.first_error.get_or_insert_with(|| error.to_string());
            }
        }
    }

    phase
}

async fn run_connections_phase(
    connections: &[BenchConnection],
    uri: Arc<str>,
    streams: usize,
    duration: Duration,
) -> Phase {
    let mut tasks = JoinSet::new();
    for (_, send_request, _) in connections {
        let send_request = send_request.clone();
        let uri = uri.clone();
        tasks.spawn(async move { run_phase(send_request, uri, streams, duration).await });
    }

    let mut phase = Phase::default();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(connection_phase) => phase.merge(connection_phase),
            Err(error) => {
                phase.errors += 1;
                phase.first_error.get_or_insert_with(|| error.to_string());
            }
        }
    }
    phase
}

async fn request_once(
    mut send_request: H3SendRequest,
    uri: Arc<str>,
) -> Result<(u64, u64), String> {
    let started = Instant::now();
    let request = Request::get(uri.as_ref())
        .body(())
        .map_err(|error| error.to_string())?;
    let mut stream = send_request
        .send_request(request)
        .await
        .map_err(|error| error.to_string())?;
    let response = stream
        .recv_response()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let mut bytes = 0_u64;
    while let Some(chunk) = stream
        .recv_data()
        .await
        .map_err(|error| error.to_string())?
    {
        bytes += chunk.remaining() as u64;
    }
    if !status.is_success() {
        return Err(format!("unexpected HTTP status {status}"));
    }
    Ok((
        started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        bytes,
    ))
}

fn client_config(config: &Config) -> Result<quinn::ClientConfig, String> {
    let mut roots = RootCertStore::empty();
    let mut reader =
        BufReader::new(File::open(&config.certificate_authority).map_err(|error| {
            format!("open {}: {error}", config.certificate_authority.display())
        })?);
    for certificate in rustls_pemfile::certs(&mut reader) {
        roots
            .add(certificate.map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    }
    if roots.is_empty() {
        return Err(format!(
            "no certificates found in {}",
            config.certificate_authority.display()
        ));
    }
    let mut crypto = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(roots)
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"h3".to_vec()];
    Ok(quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(crypto).map_err(|error| error.to_string())?,
    )))
}

async fn connect(
    endpoint: &quinn::Endpoint,
    config: &Config,
) -> Result<
    (
        quinn::Connection,
        H3SendRequest,
        tokio::task::JoinHandle<()>,
    ),
    String,
> {
    let connection = endpoint
        .connect(config.address, &config.sni)
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| error.to_string())?;
    let (mut driver, send_request) = h3::client::builder()
        .build(H3Connection::new(connection.clone()))
        .await
        .map_err(|error| error.to_string())?;
    let driver_task = tokio::spawn(async move {
        let _ = std::future::poll_fn(|context| driver.poll_close(context)).await;
    });
    Ok((connection, send_request, driver_task))
}

async fn create_connections(
    endpoint: &quinn::Endpoint,
    config: &Config,
) -> Result<Vec<BenchConnection>, String> {
    let mut connections = Vec::with_capacity(config.connections);
    for _ in 0..config.connections {
        connections.push(connect(endpoint, config).await?);
    }
    Ok(connections)
}

async fn close_connections(connections: Vec<BenchConnection>) {
    for (connection, _, driver_task) in connections {
        connection.close(quinn::VarInt::from_u32(0), b"done");
        let _ = tokio::time::timeout(Duration::from_secs(2), driver_task).await;
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("h3load: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let config = Config::parse()?;
    let client_config = client_config(&config)?;
    let mut endpoint =
        quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).map_err(|error| error.to_string())?;
    endpoint.set_default_client_config(client_config);

    let uri: Arc<str> = Arc::from(config.uri());
    println!("round\tsuccess\terrors\tbytes\tduration_s\treq_s\tp50_ms\tp99_ms\tmean_ms");
    for round in 1..=config.rounds {
        let connections = create_connections(&endpoint, &config).await?;
        if !config.warmup.is_zero() {
            let _ = run_connections_phase(&connections, uri.clone(), config.streams, config.warmup)
                .await;
        }
        let started = Instant::now();
        let mut phase =
            run_connections_phase(&connections, uri.clone(), config.streams, config.duration).await;
        let elapsed = started.elapsed().as_secs_f64();
        let requests = phase.successes + phase.errors;
        let req_s = if elapsed > 0.0 {
            phase.successes as f64 / elapsed
        } else {
            0.0
        };
        let p50_ms = phase.percentile_ms(0.50);
        let p99_ms = phase.percentile_ms(0.99);
        let mean_ms = phase.mean_ms();
        println!(
            "{round}\t{}\t{}\t{}\t{elapsed:.3}\t{req_s:.2}\t{p50_ms:.3}\t{p99_ms:.3}\t{mean_ms:.3}",
            phase.successes, phase.errors, phase.bytes,
        );
        if let Some(error) = &phase.first_error {
            eprintln!("first error: {error}");
        }
        if requests == 0 {
            return Err("no requests completed".to_string());
        }
        close_connections(connections).await;
        endpoint.wait_idle().await;
    }
    Ok(())
}
