//! Real filesystem, executable, socket and worker integration checks.
mod acceptance;
mod allocator;
mod cli;
mod common;
mod filesystem;
mod http;
mod integration;
#[cfg(target_os = "linux")]
mod lifecycle;
mod protocol;
mod runtime_audit;
mod worker;

#[global_allocator]
static ALLOCATOR: state_runtime::LimitedAllocator = state_runtime::LimitedAllocator;
