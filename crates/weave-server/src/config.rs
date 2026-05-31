use clap::Parser;
use std::path::PathBuf;

/// Weave — web-based multi-agent coordination platform.
#[derive(Parser, Debug)]
#[command(name = "weave-server", version, about)]
pub struct Config {
    /// Host address to bind to.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 3000)]
    pub port: u16,

    /// Path to the SQLite database file.
    #[arg(long, default_value = "weave.db")]
    pub db_path: PathBuf,

    /// Allow binding to non-localhost addresses.
    /// Required when --host is set to 0.0.0.0 or another non-loopback address.
    #[arg(long, default_value_t = false)]
    pub allow_remote: bool,
}
