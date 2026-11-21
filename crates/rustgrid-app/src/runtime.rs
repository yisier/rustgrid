use std::future::Future;

pub struct Runtime {
    inner: tokio::runtime::Runtime,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    pub fn new() -> Self {
        let inner = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime");
        Self { inner }
    }

    pub fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.inner.handle().spawn(future)
    }

    /// Run a blocking task (file I/O, spreadsheet parsing) on tokio's blocking pool, so it never
    /// stalls the async workers.
    pub fn spawn_blocking<F, R>(&self, function: F) -> tokio::task::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.inner.handle().spawn_blocking(function)
    }
}
