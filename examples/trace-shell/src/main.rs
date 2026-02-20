mod handlers;
mod shell;
mod state;

use clap::Parser;
use state::SharedState;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Parser)]
#[command(
    name = "trace-shell",
    about = "Interactive trace-forward protocol shell.\n\n\
             Connects to a cardano-tracer as a simulated node and lets you\n\
             craft custom trace objects, metrics, and datapoints."
)]
struct Args {
    /// Connect to this socket on startup (optional).
    #[arg(short, long)]
    socket: Option<String>,

    /// Protocol magic (default: mainnet).
    #[arg(long, default_value_t = 764824073)]
    magic: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Default to WARN to avoid handler log noise interleaving with the shell.
    // Override with RUST_LOG=info or RUST_LOG=debug for protocol-level output.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .init();

    let args = Args::parse();
    let state: SharedState = Arc::new(RwLock::new(state::State::new()));

    println!("trace-shell: Interactive trace-forward protocol shell");
    println!("Type 'help' for available commands.");
    println!();

    // If socket provided on command line, auto-connect before entering the shell
    if let Some(ref socket) = args.socket {
        let connect_args = if args.magic != 764824073 {
            vec![
                socket.clone(),
                "--magic".to_string(),
                args.magic.to_string(),
            ]
        } else {
            vec![socket.clone()]
        };
        // We need a mutable ConnectionState for the shell to manage.
        // Rather than duplicating connect logic, synthesize a "connect" command
        // that the shell will process first. But the shell's run_shell owns
        // ConnectionState. So we pass the auto-connect info into run_shell.
        //
        // For simplicity, just print a hint and let the shell handle it.
        println!("Auto-connect: will connect to {} on first prompt.", socket);
        // We'll pre-feed the connect command by writing it before the shell loop.
        // Actually the cleanest approach is to just call the shell and have it
        // handle an initial connect. Let's pass the args through.
        return shell::run_shell_with_autoconnect(state, Some(connect_args)).await;
    }

    shell::run_shell_with_autoconnect(state, None).await
}
