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

    #[allow(dead_code)]
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.inner.handle().clone()
    }
}
