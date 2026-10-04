mod cli;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Command, Storage, ToolCommand};
use rmcp::{ServiceExt, transport::stdio};
use statemcp::{Server, State, Store, WorkerConfig, http::HttpOptions};
use std::{env, process::ExitCode, sync::Arc};
use tracing_subscriber::EnvFilter;

#[global_allocator]
static ALLOCATOR: state_runtime::LimitedAllocator = state_runtime::LimitedAllocator;

fn open_store(storage: &Storage) -> Result<Store> {
    Store::open(&storage.data_dir)
        .with_context(|| format!("cannot open state at {}", storage.data_dir.display()))
}

fn open_state(storage: &Storage) -> Result<State> {
    let executable = env::current_exe().context("cannot locate the worker executable")?;
    Ok(State::with_backend(
        open_store(storage)?,
        Arc::new(WorkerConfig::new(executable)),
    ))
}

fn invoke(tool: ToolCommand) -> Result<ExitCode> {
    // Load inputs before opening the store, so invalid inputs do not create it.
    let request = match tool.request() {
        Ok(request) => request,
        Err(error) => {
            if let Some(error) = error.downcast_ref::<statemcp::CoreError>() {
                eprintln!("{}", serde_json::json!({"error": error}));
                return Ok(ExitCode::FAILURE);
            }
            return Err(error);
        }
    };
    let state = open_state(tool.storage())?;
    match state.dispatch_request(request) {
        Ok(value) => {
            println!("{value}");
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            // Preserve machine-readable application errors for direct CLI callers.
            eprintln!("{}", serde_json::json!({"error": error}));
            Ok(ExitCode::FAILURE)
        }
    }
}

async fn serve(command: Command) -> Result<()> {
    match command {
        Command::Stdio { storage } => {
            let server = Server::new(open_state(&storage)?)
                .serve(stdio())
                .await
                .context("cannot initialize MCP over stdio")?;
            server.waiting().await.context("stdio server failed")?;
        }
        Command::Http {
            storage,
            bind,
            auth_bearer,
        } => {
            let listener = tokio::net::TcpListener::bind(bind)
                .await
                .with_context(|| format!("cannot listen on {bind}"))?;
            let address = listener.local_addr()?;
            let options = HttpOptions::new(address, auth_bearer)?;
            let state = open_state(&storage)?;
            tracing::info!(url = %format!("http://{address}/mcp"), "MCP server listening");
            statemcp::http::serve(listener, state, options)
                .await
                .context("HTTP server failed")?;
        }
        _ => unreachable!("only transport commands start a server"),
    }
    Ok(())
}

fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Worker {
            memory_bytes,
            max_frame_bytes,
        } => state_runtime::worker_main(memory_bytes, max_frame_bytes)
            .context("Monty worker failed")?,
        Command::Maintenance {
            storage,
            retain_receipts,
        } => {
            let report = open_store(&storage)?.maintenance(retain_receipts)?;
            println!("{}", serde_json::to_string(&report)?);
        }
        Command::Cli { tool } => return invoke(tool),
        command => tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("cannot start the async runtime")?
            .block_on(serve(command))?,
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Worker mode must start before logging or the async runtime is initialized.
    let worker = matches!(cli.command, Command::Worker { .. });
    if !worker {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("warn,statemcp=info")),
            )
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .init();
    }
    match run(cli.command) {
        Ok(status) => status,
        Err(error) => {
            if worker {
                eprintln!("statemcp: {error:#}");
            } else {
                tracing::error!(error = %format!("{error:#}"), "statemcp failed");
            }
            ExitCode::FAILURE
        }
    }
}
