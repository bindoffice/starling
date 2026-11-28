use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{
    ConfigLoader, LogDestination, LogLevel, LogTarget, RuntimeConfig, WorkerProcesses, compile,
};
use crate::logging::Logger;
use crate::server;
use crate::{SERVER_SOFTWARE, STARLING_VERSION};

const CONFIGURE_ARGUMENTS: &str = "--with-http_starling_module --with-http_ssl_module --with-http_v2_module \
     --with-stream --with-stream_ssl_module";
const WORKER_THREADS_PER_RUNTIME: usize = 4;

// Match the usual Linux/Nginx worker stack. Nested internal redirects,
// auth_request subrequests, and error-page handling can exceed 4 MiB.
pub(crate) const WORKER_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Cli {
    pub prefix: PathBuf,
    pub config: PathBuf,
    pub error_log: Option<String>,
    pub globals: String,
    pub test: bool,
    pub dump: bool,
    pub quiet: bool,
    pub signal: Option<String>,
    pub show_version: bool,
    pub show_help: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            prefix: PathBuf::from("/usr/local/nginx"),
            config: PathBuf::from("conf/nginx.conf"),
            error_log: None,
            globals: String::new(),
            test: false,
            dump: false,
            quiet: false,
            signal: None,
            show_version: false,
            show_help: false,
        }
    }
}

pub fn parse_cli(arguments: impl IntoIterator<Item = String>) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            break;
        }
        if !argument.starts_with('-') || argument == "-" {
            return Err(format!("invalid option: {argument}"));
        }
        if argument.starts_with("--") {
            match argument.as_str() {
                "--help" => cli.show_help = true,
                "--version" => cli.show_version = true,
                "--test" => cli.test = true,
                _ => return Err(format!("invalid option: {argument}")),
            }
            continue;
        }
        let mut chars = argument[1..].chars().peekable();
        while let Some(option) = chars.next() {
            match option {
                '?' | 'h' => cli.show_help = true,
                'v' => cli.show_version = true,
                'V' => {
                    cli.show_version = true;
                    cli.dump = true;
                }
                't' => cli.test = true,
                'T' => {
                    cli.test = true;
                    cli.dump = true;
                }
                'q' => cli.quiet = true,
                'p' | 'c' | 'e' | 'g' | 's' => {
                    let attached: String = chars.collect();
                    let value = if attached.is_empty() {
                        arguments
                            .next()
                            .ok_or_else(|| format!("option \"-{option}\" requires parameter"))?
                    } else {
                        attached
                    };
                    match option {
                        'p' => cli.prefix = PathBuf::from(value),
                        'c' => cli.config = PathBuf::from(value),
                        'e' => cli.error_log = Some(value),
                        'g' => {
                            if !cli.globals.is_empty() {
                                cli.globals.push('\n');
                            }
                            cli.globals.push_str(&value);
                        }
                        's' => cli.signal = Some(value),
                        _ => unreachable!(),
                    }
                    break;
                }
                other => return Err(format!("invalid option: -{other}")),
            }
        }
    }
    Ok(cli)
}

pub fn load(cli: &Cli) -> Result<RuntimeConfig, String> {
    let prefix = absolute_path(&cli.prefix);
    let config_path = resolve_config_path(&cli.config, &prefix);
    let loader = ConfigLoader::new(prefix.clone());
    let document = loader
        .load_with_inline(&config_path, &cli.globals)
        .map_err(|err| err.to_string())?;
    let files = document.files.clone();
    let mut config =
        compile(document, prefix.clone(), config_path.clone()).map_err(|err| err.to_string())?;
    if let Some(error_log) = &cli.error_log {
        let level = config
            .core
            .error_logs
            .first()
            .map(|target| target.level)
            .unwrap_or(LogLevel::Error);
        config.core.error_logs = vec![LogTarget {
            destination: if error_log == "stderr" {
                LogDestination::Stderr
            } else {
                LogDestination::File(resolve(&prefix, error_log))
            },
            level,
        }];
    }
    config.files = files;
    Ok(config)
}

pub fn execute(cli: Cli) -> Result<(), String> {
    if cli.show_help {
        print_help();
        return Ok(());
    }
    if cli.show_version {
        print_version(cli.dump);
        if !cli.test && cli.signal.is_none() {
            return Ok(());
        }
    }

    let config = load(&cli)?;
    if !cli.quiet {
        print_config_warnings(&config);
    }
    if cli.test {
        if cli.dump {
            for file in &config.files {
                println!("# configuration file {}:", file.display());
                match fs::read_to_string(file) {
                    Ok(contents) => println!("{contents}"),
                    Err(_) if file == Path::new("<command line>") => {
                        println!("{}", cli.globals);
                    }
                    Err(err) => {
                        return Err(format!(
                            "starling: [emerg] could not read {}: {err}",
                            file.display()
                        ));
                    }
                }
            }
        }
        if !cli.quiet {
            println!(
                "starling: the configuration file {} syntax is ok",
                config.config_path.display()
            );
            println!(
                "starling: configuration file {} test is successful",
                config.config_path.display()
            );
        }
        return Ok(());
    }

    if let Some(signal) = &cli.signal {
        return send_signal(signal, &config.core.pid);
    }

    let logger = Arc::new(
        Logger::new(&config.core.error_logs, &config.prefix)
            .map_err(|err| format!("open() error log failed: {err}"))?,
    );
    let pid_path = config.core.pid.clone();
    let result = start_server(config, logger.clone());
    if let Err(error) = &result {
        logger.log(LogLevel::Emerg, format!("startup failed: {error}"));
    }
    remove_pid(&pid_path);
    result
}

fn start_server(config: RuntimeConfig, logger: Arc<Logger>) -> Result<(), String> {
    if config.core.daemon {
        daemonize()?;
    }

    if let Some(directory) = &config.core.working_directory {
        std::env::set_current_dir(directory)
            .map_err(|err| format!("chdir(\"{}\") failed: {err}", directory.display()))?;
    }

    write_pid(&config.core.pid)
        .map_err(|err| format!("open() \"{}\" failed: {err}", config.core.pid.display()))?;

    logger.log(
        LogLevel::Notice,
        format!(
            "using the \"epoll\" event method; starling {}",
            STARLING_VERSION
        ),
    );

    run_async(config, logger)
}

fn print_config_warnings(config: &RuntimeConfig) {
    for warning in &config.warnings {
        eprintln!("starling: [warn] {warning}");
    }
}

fn run_async(config: RuntimeConfig, logger: Arc<Logger>) -> Result<(), String> {
    let worker_threads = match config.core.worker_processes {
        WorkerProcesses::Auto => std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
        WorkerProcesses::Count(count) => count.max(1),
    };
    let worker_layout = worker_runtime_layout(worker_threads);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .thread_name("starling-coordinator")
        .build()
        .map_err(|err| format!("failed to create async runtime: {err}"))?;
    runtime
        .block_on(server::run(
            config,
            logger,
            worker_layout,
            blocking_thread_limit(worker_threads),
        ))
        .map_err(|err| format!("runtime error: {err}"))
}

fn worker_runtime_layout(worker_threads: usize) -> Vec<usize> {
    let worker_threads = worker_threads.max(1);
    let mut remaining = worker_threads;
    let mut layout = Vec::new();
    while remaining > 0 {
        let threads = remaining.min(WORKER_THREADS_PER_RUNTIME);
        layout.push(threads);
        remaining -= threads;
    }
    layout
}

fn blocking_thread_limit(worker_threads: usize) -> usize {
    // Buffered request bodies can keep ingress spooling and proxy replay in
    // flight together; bound their blocking I/O instead of growing one OS
    // thread per concurrent file operation.
    worker_threads.saturating_mul(2).clamp(8, 64)
}

#[cfg(unix)]
fn send_signal(signal: &str, pid_path: &Path) -> Result<(), String> {
    use rustix::process::Signal;

    let signal = match signal {
        "stop" => Signal::TERM,
        "quit" => Signal::QUIT,
        "reload" => Signal::HUP,
        "reopen" => Signal::USR1,
        _ => {
            return Err(format!(
                "invalid option: \"-s {signal}\" (expected stop, quit, reload or reopen)"
            ));
        }
    };
    let contents = fs::read_to_string(pid_path)
        .map_err(|err| format!("open() \"{}\" failed: {err}", pid_path.display()))?;
    let pid = contents
        .trim()
        .parse::<i32>()
        .map_err(|_| format!("invalid PID number in \"{}\"", pid_path.display()))?;
    let pid = rustix::process::Pid::from_raw(pid)
        .ok_or_else(|| format!("invalid PID number in \"{}\"", pid_path.display()))?;
    rustix::process::kill_process(pid, signal).map_err(|error| {
        format!(
            "kill({}, {}) failed: {error}",
            pid.as_raw_pid(),
            signal.as_raw()
        )
    })
}

#[cfg(not(unix))]
fn send_signal(_signal: &str, _pid_path: &Path) -> Result<(), String> {
    Err("sending signals is only supported on Unix".into())
}

fn write_pid(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    writeln!(file, "{}", std::process::id())?;
    file.flush()
}

fn remove_pid(path: &Path) {
    if let Ok(contents) = fs::read_to_string(path)
        && contents.trim() == std::process::id().to_string()
    {
        let _ = fs::remove_file(path);
    }
}

fn daemonize() -> Result<(), String> {
    #[cfg(unix)]
    {
        // SAFETY: daemonize runs before the Tokio runtime and worker threads are
        // created, so fork does not duplicate a multithreaded process here.
        let first = unsafe { libc::fork() };
        if first < 0 {
            return Err(format!("fork() failed: {}", io::Error::last_os_error()));
        }
        if first > 0 {
            std::process::exit(0);
        }
        rustix::process::setsid().map_err(|error| format!("setsid() failed: {error}"))?;
        // SAFETY: the process is still single-threaded after the first fork.
        let second = unsafe { libc::fork() };
        if second < 0 {
            return Err(format!("fork() failed: {}", io::Error::last_os_error()));
        }
        if second > 0 {
            std::process::exit(0);
        }
        rustix::process::umask(rustix::fs::Mode::empty());
        let null = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .map_err(|err| format!("open(\"/dev/null\") failed: {err}"))?;
        rustix::stdio::dup2_stdin(&null)
            .map_err(|error| format!("dup2(STDIN_FILENO) failed: {error}"))?;
        rustix::stdio::dup2_stdout(&null)
            .map_err(|error| format!("dup2(STDOUT_FILENO) failed: {error}"))?;
        rustix::stdio::dup2_stderr(&null)
            .map_err(|error| format!("dup2(STDERR_FILENO) failed: {error}"))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err("daemon mode is only supported on Unix".into())
    }
}

fn print_version(verbose: bool) {
    println!("{}", version_summary());
    if verbose {
        println!("built by rustc");
        println!("TLS SNI support enabled");
        println!("configure arguments: {CONFIGURE_ARGUMENTS}");
    }
}

fn print_help() {
    println!(
        "{}\n\
         Usage: starling [-?hvVtTq] [-s signal] [-p prefix]\n\
         \t\t[-e filename] [-c filename] [-g directives]\n\n\
         Options:\n\
         \t-?,-h         : this help\n\
         \t-v            : show version and exit\n\
         \t-V            : show version and configure options then exit\n\
         \t-t            : test configuration and exit\n\
         \t-T            : test configuration, dump it and exit\n\
         \t-q            : suppress non-error messages during configuration testing\n\
         \t-s signal     : send signal to a master process: stop, quit, reload, reopen\n\
         \t-p prefix     : set prefix path (default: /usr/local/nginx/)\n\
         \t-e filename   : set error log file (default: logs/error.log)\n\
         \t-c filename   : set configuration file (default: conf/nginx.conf)\n\
         \t-g directives : set global directives out of configuration file",
        version_summary()
    );
}

fn version_summary() -> String {
    format!("starling version: {STARLING_VERSION} (nginx-compatible: {SERVER_SOFTWARE})")
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn resolve_config_path(config: &Path, prefix: &Path) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_config_path_from(config, prefix, &cwd)
}

fn resolve_config_path_from(config: &Path, prefix: &Path, cwd: &Path) -> PathBuf {
    if config.is_absolute() {
        return config.to_path_buf();
    }
    // Keep nginx's prefix-relative default, but let an explicit or fallback
    // working-directory path work without requiring -p to match the config.
    if matches!(
        config.components().next(),
        Some(std::path::Component::CurDir | std::path::Component::ParentDir)
    ) {
        return cwd.join(config);
    }
    let prefixed = prefix.join(config);
    if prefixed.exists() {
        return prefixed;
    }
    let cwd_relative = cwd.join(config);
    if cwd_relative.exists() {
        return cwd_relative;
    }
    prefixed
}

fn resolve(prefix: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        prefix.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn parses_nginx_style_arguments() {
        let cli = parse_cli(
            ["-t", "-p", "/tmp/nginx", "-c", "custom.conf"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert!(cli.test);
        assert_eq!(cli.prefix, PathBuf::from("/tmp/nginx"));
        assert_eq!(cli.config, PathBuf::from("custom.conf"));
    }

    #[test]
    fn resolves_relative_config_paths_from_prefix_or_cwd() {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "starling-runtime-config-path-{}-{id}",
            std::process::id()
        ));
        let prefix = root.join("prefix");
        let cwd = root.join("cwd");
        fs::create_dir_all(&prefix).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        let cwd_config = cwd.join("nginx.conf");
        fs::write(&cwd_config, "events {}\n").unwrap();
        assert_eq!(
            resolve_config_path_from(Path::new("nginx.conf"), &prefix, &cwd),
            cwd_config
        );

        let prefix_config = prefix.join("nginx.conf");
        fs::write(&prefix_config, "events {}\n").unwrap();
        assert_eq!(
            resolve_config_path_from(Path::new("nginx.conf"), &prefix, &cwd),
            prefix_config
        );
        assert_eq!(
            resolve_config_path_from(Path::new("./nginx.conf"), &prefix, &cwd),
            cwd.join("./nginx.conf")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bounds_blocking_threads_for_large_worker_counts() {
        assert_eq!(blocking_thread_limit(1), 8);
        assert_eq!(blocking_thread_limit(16), 32);
        assert_eq!(blocking_thread_limit(64), 64);
    }

    #[test]
    fn partitions_worker_threads_into_bounded_runtimes() {
        assert_eq!(worker_runtime_layout(1), vec![1]);
        assert_eq!(worker_runtime_layout(2), vec![2]);
        assert_eq!(worker_runtime_layout(3), vec![3]);
        assert_eq!(worker_runtime_layout(16), vec![4; 4]);
    }

    #[test]
    fn reports_http2_in_configure_arguments() {
        assert!(CONFIGURE_ARGUMENTS.contains("--with-http_v2_module"));
    }

    #[test]
    fn version_summary_uses_starling_branding() {
        let summary = version_summary();
        assert!(summary.starts_with("starling version:"), "{summary}");
        assert!(summary.contains(STARLING_VERSION), "{summary}");
        assert!(summary.contains(SERVER_SOFTWARE), "{summary}");
    }

    #[test]
    fn records_startup_failures_in_the_error_log() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix = std::env::temp_dir().join(format!(
            "starling-runtime-startup-failure-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(prefix.join("conf")).unwrap();
        fs::write(
            prefix.join("conf/nginx.conf"),
            format!(
                "daemon off;\n\
                 auto_https off;\n\
                 error_log logs/error.log notice;\n\
                 events {{}}\n\
                 http {{ server {{ listen 127.0.0.1:{port}; }} }}\n"
            ),
        )
        .unwrap();

        let mut cli = Cli::default();
        cli.prefix = prefix.clone();
        cli.config = PathBuf::from("conf/nginx.conf");
        cli.quiet = true;
        let error = execute(cli).unwrap_err();
        assert!(error.contains("bind()"), "{error}");

        let log = fs::read_to_string(prefix.join("logs/error.log")).unwrap();
        assert!(log.contains("[emerg]"), "{log}");
        assert!(log.contains("startup failed"), "{log}");
        assert!(log.contains("bind()"), "{log}");
        assert!(!prefix.join("logs/nginx.pid").exists());

        let _ = fs::remove_dir_all(prefix);
    }
}
