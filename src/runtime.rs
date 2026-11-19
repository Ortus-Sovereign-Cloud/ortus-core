//! The runtime that Ortus does its input and output on.
//!
//! Storage, files and jobs are async. Code that is not async calls them
//! through the `blocking` modules, which wait on this runtime. There is one
//! for the whole process and it is never shut down: database and HTTP
//! connections are kept open on it between calls.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Handle, Runtime};

/// The process-wide I/O runtime.
pub fn io() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .thread_name("ortus-io")
            .build()
            .expect("ortus: could not start the I/O runtime")
    })
}

/// Waits for `future` from code that is not async.
///
/// # Panics
///
/// Panics when called from async code, where waiting would stall the
/// runtime. Async code awaits the async interfaces directly.
pub fn block_on<F: Future>(future: F) -> F::Output {
    assert!(
        Handle::try_current().is_err(),
        "a blocking Ortus call was made from async code; await the async interface instead"
    );
    io().block_on(future)
}
