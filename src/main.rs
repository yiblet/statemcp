use state_mcp::{Server, State, Store, WorkerConfig};
use std::{env, io, path::PathBuf, process::ExitCode, sync::Arc};

#[global_allocator]
static ALLOCATOR: state_runtime::LimitedAllocator = state_runtime::LimitedAllocator;

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let mut data_dir = PathBuf::from(".state-mcp");
    let mut maintenance = false;
    let mut retain_receipts = 10000;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--worker") => {
                let memory = args
                    .next()
                    .and_then(|s| s.to_str().and_then(|s| s.parse().ok()))
                    .ok_or("invalid worker memory limit")?;
                let frame = args
                    .next()
                    .and_then(|s| s.to_str().and_then(|s| s.parse().ok()))
                    .ok_or("invalid worker frame limit")?;
                if args.next().is_some() {
                    return Err("unexpected worker argument".into());
                }
                return state_runtime::worker_main(memory, frame).map_err(|e| e.to_string());
            }
            Some("--help" | "-h") => {
                println!(
                    "state-mcp {}\n\nStateful agent APIs over MCP stdio.\n\nUSAGE:\n    state-mcp [--data-dir PATH]\n\nOPTIONS:\n    --data-dir PATH  Managed state directory (default: .state-mcp)\n    --maintenance   Run explicit storage cleanup and exit\n    --retain-receipts N  Receipts retained by cleanup (default: 10000)\n    -h, --help       Print help\n    -V, --version    Print version",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            Some("--version" | "-V") => {
                println!("state-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("--maintenance") => maintenance = true,
            Some("--retain-receipts") => {
                retain_receipts = args
                    .next()
                    .and_then(|s| s.to_str().and_then(|s| s.parse::<usize>().ok()))
                    .ok_or("--retain-receipts requires a nonnegative integer")?;
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
    let store = Store::open(data_dir).map_err(|error| error.to_string())?;
    if maintenance {
        let report = store
            .maintenance(retain_receipts)
            .map_err(|error| error.to_string())?;
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    // TODO(audit): assess aggregate memory across nested workers; current cap is per worker.
    // TODO(audit): evaluate OS sandboxing; workers currently provide crash isolation.
    // TODO(audit): support preempting host callbacks; watchdogs only interrupt Monty.
    // TODO(performance): profile worker startup overhead and optional worker reuse.
    let state = State::with_backend(store, Arc::new(WorkerConfig::new(executable)));
    Server::new(state)
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
