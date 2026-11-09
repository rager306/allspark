pub struct Worker {
    pub id: u32,
}

impl Worker {
    /// One of the two `run` methods in this crate.
    pub fn run(&self) -> u32 {
        self.id * 2
    }
}

pub struct Backup {
    pub id: u32,
}

impl Backup {
    /// The other one. Its body is textually identical to `Worker::run`'s, so
    /// `grep "run()"` finds both call sites and cannot say which receiver each
    /// belongs to.
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
