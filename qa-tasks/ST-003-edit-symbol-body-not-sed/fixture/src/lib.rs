pub struct Config {
    pub retries: u32,
    pub timeout_ms: u32,
}

impl Config {
    /// Replaced wholesale by the task.
    pub fn describe(&self) -> String {
        format!("retries={} timeout={}", self.retries, self.timeout_ms)
    }

    /// Must stay byte-identical: it shares the same body shape.
    pub fn summary(&self) -> String {
        format!("retries={} timeout={}", self.retries, self.timeout_ms)
    }
}
