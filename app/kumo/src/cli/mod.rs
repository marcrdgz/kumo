pub mod bindings;
pub mod chrome;
#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod client_view;
// The control CLI lives in `cli/cli.rs` (the `cli` folder hosts the whole
// client); the name collision is intentional.
#[cfg(unix)]
#[allow(clippy::module_inception)]
pub mod cli;
pub mod mouse;
pub mod util;
