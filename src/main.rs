use state_mcp::{Server, UnsupportedDispatcher};
use std::{env, io, path::PathBuf, process::ExitCode};

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let mut data_dir = PathBuf::from(".state-mcp");
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "state-mcp {}\n\nStateful agent APIs over MCP stdio.\n\nUSAGE:\n    state-mcp [--data-dir PATH]\n\nOPTIONS:\n    --data-dir PATH  Managed state directory (default: .state-mcp)\n    -h, --help       Print help\n    -V, --version    Print version",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            Some("--version" | "-V") => {
                println!("state-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("--data-dir") => {
                data_dir = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or("--data-dir requires a path")?;
            }
            _ => return Err(format!("unknown argument: {}", arg.to_string_lossy())),
        }
    }
    // The core integration will open State at this path and use it as dispatcher.
    let _ = data_dir;
    Server::new(UnsupportedDispatcher)
        .serve(io::stdin().lock(), io::stdout().lock())
        .map_err(|error| error.to_string())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("state-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}
