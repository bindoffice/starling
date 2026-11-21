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

pub(crate) const WORKER_THREAD_STACK_SIZE: usize = 4 * 1024 * 1024;

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
    let config_path = if cli.config.is_absolute() {
        cli.config.clone()
    } else {
        prefix.join(&cli.config)
    };
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
                            "nginx: [emerg] could not read {}: {err}",
                            file.display()
                        ));
                    }
                }
            }
        }
        if !cli.quiet {
            println!(
                "nginx: the configuration file {} syntax is ok",
                config.config_path.display()
            );
            println!(
                "nginx: configuration file {} test is successful",
                config.config_path.display()
            );
        }
        return Ok(());
    }

    if let Some(signal) = &cli.signal {
        return send_signal(signal, &config.core.pid);
    }

    if config.core.daemon {
        daemonize()?;
    }

    if let Some(directory) = &config.core.working_directory {
        std::env::set_current_dir(directory)
            .map_err(|err| format!("chdir(\"{}\") failed: {err}", directory.display()))?;
    }

    write_pid(&config.core.pid)
        .map_err(|err| format!("open() \"{}\" failed: {err}", config.core.pid.display()))?;

    let logger = Arc::new(
        Logger::new(&config.core.error_logs, &config.prefix)
            .map_err(|err| format!("open() error log failed: {err}"))?,
    );
    logger.log(
        crate::config::LogLevel::Notice,
        format!(
            "using the \"epoll\" event method; starling {}",
            STARLING_VERSION
        ),
    );

    let pid_path = config.core.pid.clone();
    let result = run_async(config, logger);
    if result.is_ok() {
        remove_pid(&pid_path);
    }
    result
}

fn print_config_warnings(config: &RuntimeConfig) {
    for warning in &config.warnings {
        eprintln!("nginx: [warn] {warning}");
    }
}

fn run_async(config: RuntimeConfig, logger: Arc<Logger>) -> Result<(), String> {
    let worker_threads = match config.core.worker_processes {
        WorkerProcesses::Auto => std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
        WorkerProcesses::Count(count) => count.max(1),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .thread_stack_size(WORKER_THREAD_STACK_SIZE)
        .thread_name("starling-worker")
        .build()
        .map_err(|err| format!("failed to create async runtime: {err}"))?;
    runtime
        .block_on(server::run(config, logger))
        .map_err(|err| format!("runtime error: {err}"))
}

fn send_signal(signal: &str, pid_path: &Path) -> Result<(), String> {
    let signal_number = match signal {
        "stop" => libc::SIGTERM,
        "quit" => libc::SIGQUIT,
        "reload" => libc::SIGHUP,
        "reopen" => libc::SIGUSR1,
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
    let result = unsafe { libc::kill(pid, signal_number) };
    if result != 0 {
        return Err(format!(
            "kill({pid}, {signal_number}) failed: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
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
        let first = unsafe { libc::fork() };
        if first < 0 {
            return Err(format!("fork() failed: {}", io::Error::last_os_error()));
        }
        if first > 0 {
            std::process::exit(0);
        }
        if unsafe { libc::setsid() } < 0 {
            return Err(format!("setsid() failed: {}", io::Error::last_os_error()));
        }
        let second = unsafe { libc::fork() };
        if second < 0 {
            return Err(format!("fork() failed: {}", io::Error::last_os_error()));
        }
        if second > 0 {
            std::process::exit(0);
        }
        unsafe {
            libc::umask(0);
        }
        let null = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .map_err(|err| format!("open(\"/dev/null\") failed: {err}"))?;
        unsafe {
            libc::dup2(null.as_raw_fd(), libc::STDIN_FILENO);
            libc::dup2(null.as_raw_fd(), libc::STDOUT_FILENO);
            libc::dup2(null.as_raw_fd(), libc::STDERR_FILENO);
        }
        std::mem::forget(null);
        Ok(())
    }
    #[cfg(not(unix))]
    {
        Err("daemon mode is only supported on Unix".into())
    }
}

#[cfg(unix)]
use std::os::fd::AsRawFd;

fn print_version(verbose: bool) {
    println!("nginx version: {SERVER_SOFTWARE} (starling/{STARLING_VERSION})");
    if verbose {
        println!("built by rustc");
        println!("configure arguments: --with-http_starling_module");
    }
}

fn print_help() {
    println!(
        "nginx version: {SERVER_SOFTWARE} (starling/{STARLING_VERSION})\n\
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
         \t-g directives : set global directives out of configuration file"
    );
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
}
