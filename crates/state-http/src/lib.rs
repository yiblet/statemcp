//! Async hosting boundary for synchronous namespace application services.
mod blocking;
pub use blocking::{BlockingError, BlockingExecutor};

mod hosting;
pub use hosting::{Hosting, RequestOrigin, namespace_router};

mod transfers;
pub use transfers::{Transfers, transfer_router};
