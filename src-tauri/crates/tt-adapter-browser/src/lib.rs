//! Opt-in, loopback HTTP transport. No Tauri, application services, or data owner.
//! Host requests cross a bounded channel; all business execution stays in the host.

mod server;

pub use server::{Config, HostRequest, Server, bind, cached_response};
