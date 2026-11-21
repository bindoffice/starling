use super::*;

pub(super) fn compile_events(directive: &Directive, core: &mut CoreConfig) -> ConfigResult<()> {
    for child in &directive.children {
        match child.name.as_str() {
            "worker_connections" => {
                core.worker_connections =
                    parse_usize(one_arg(child)?, &child.location, "worker_connections")?;
            }
            "use"
            | "multi_accept"
            | "accept_mutex"
            | "accept_mutex_delay"
            | "debug_connection"
            | "worker_aio_requests" => {
                validate_no_block(child)?;
            }
            other => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("unknown directive \"{other}\" in events context"),
                ));
            }
        }
    }
    Ok(())
}
