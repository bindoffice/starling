use super::support::*;
use super::*;

const LISTEN_PORT: u16 = 18920;
const BODY_SIZE: usize = 4 * 1024 * 1024;
const SLOW_BODY_SIZE: usize = 1024 * 1024;

mod fixtures;
mod half_close;
mod keepalive;
mod midflight_delete;
mod mixed_pressure;
mod slow_network;
mod spill_cancellation;
mod timeout_retry;
