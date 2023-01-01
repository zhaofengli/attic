//! Misc utilities.

use std::future::Future;
use std::mem;

use tokio::task;

/// Runs a future when dropped.
///
/// This is used to clean up external resources that are
/// difficult to correctly model using ownerships.
pub struct Finally<F: Future + Send + 'static>
where
    F::Output: Send + 'static,
{
    f: Option<F>,
}

impl<F: Future + Send + 'static> Finally<F>
where
    F::Output: Send + 'static,
{
    pub fn new(f: F) -> Self {
        Self { f: Some(f) }
    }

    pub fn cancel(self) {
        mem::forget(self);
    }
}

impl<F: Future + Send + 'static> Drop for Finally<F>
where
    F::Output: Send + 'static,
{
    fn drop(&mut self) {
        task::spawn(self.f.take().unwrap());
    }
}
