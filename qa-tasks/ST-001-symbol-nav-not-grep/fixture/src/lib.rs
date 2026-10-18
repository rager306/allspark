pub struct Worker {
    pub id: u32,
}

impl Worker {
    /// The method the task cares about: it is called from `spawn_worker`.
    pub fn run(&self) -> u32 {
        self.id * 2
    }
}

pub struct Backup {
    pub id: u32,
}

impl Backup {
    /// Same name, never called anywhere: a text search cannot tell them apart.
    pub fn run(&self) -> u32 {
        self.id * 3
    }
}

pub fn spawn_worker(id: u32) -> u32 {
    let worker = Worker { id };
    worker.run()
}

pub fn unrelated() -> u32 {
    let backup = Backup { id: 7 };
    backup.run()
}
