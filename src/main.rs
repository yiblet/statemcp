mod cli;
use clap::Parser;
use cli::{Cli, Command};
use statemcp::{Server, State, Store, WorkerConfig};
use std::{env, io, process::ExitCode, sync::Arc};

#[global_allocator]
static ALLOCATOR: state_runtime::LimitedAllocator = state_runtime::LimitedAllocator;

fn run() -> Result<ExitCode, String> {
    // Worker entry must run before creating state or the HTTP runtime.
    let mut args = env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--worker")) {
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
        return state_runtime::worker_main(memory, frame)
            .map(|()| ExitCode::SUCCESS)
            .map_err(|e| e.to_string());
    }
    let cli = Cli::parse();
    let request = cli.command.request()?;
    // Validate credentials before creating the storage directory.
    if let Command::Http { auth_bearer, .. } = &cli.command {
        statemcp::http::validate_bearer(auth_bearer.as_deref())?;
    }
    let store = Store::open(&cli.command.storage().data_dir).map_err(|error| error.to_string())?;
    if let Command::Maintenance {
        retain_receipts, ..
    } = cli.command
    {
        let report = store
            .maintenance(retain_receipts)
            .map_err(|error| error.to_string())?;
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|e| e.to_string())?
        );
        return Ok(ExitCode::SUCCESS);
    }
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    // TODO(audit): assess aggregate memory across nested workers; current cap is per worker.
    // TODO(audit): evaluate OS sandboxing; workers currently provide crash isolation.
    // TODO(audit): support preempting host callbacks; watchdogs only interrupt Monty.
    // TODO(performance): profile worker startup overhead and optional worker reuse.
    let state = State::with_backend(store, Arc::new(WorkerConfig::new(executable)));
    if let Some((tool, arguments)) = request {
        return match state.dispatch(&tool, arguments) {
            Ok(value) => {
                println!("{value}");
                Ok(ExitCode::SUCCESS)
            }
            Err(error) => {
                eprintln!("{}", serde_json::json!({"error":error}));
                Ok(ExitCode::FAILURE)
            }
        };
    }
    match cli.command {
        Command::Stdio { .. } => Server::new(state)
            .serve(io::stdin().lock(), io::stdout().lock())
            .map_err(|error| error.to_string()),
        Command::Http {
            bind, auth_bearer, ..
        } => tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?
            .block_on(async move {
                let listener = tokio::net::TcpListener::bind(bind).await?;
                let address = listener.local_addr()?;
                let options = statemcp::http::HttpOptions::new(address, auth_bearer)
                    .map_err(io::Error::other)?;
                eprintln!("statemcp: listening on http://{address}/mcp");
                axum::serve(listener, statemcp::http::router(state, options))
                    .with_graceful_shutdown(async {
                        let _ = tokio::signal::ctrl_c().await;
                    })
                    .await
            })
            .map_err(|e: io::Error| e.to_string()),
        _ => unreachable!("CLI operations handled above"),
    }
    .map(|()| ExitCode::SUCCESS)
}
fn main() -> ExitCode {
    match run() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("statemcp: {error}");
            ExitCode::FAILURE
        }
    }
}
