use super::support::*;
use super::*;

fn stream_config(body: &str) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                {body}
            }}
        "#
    )
}

mod config;
mod tcp;
mod udp;
