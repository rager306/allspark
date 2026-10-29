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
    /// Same name as `Worker::run`, and it IS called — from `unrelated`. The body is
    /// textually identical to `Worker::run`'s, so `grep "run()"` finds both call sites and
    /// cannot say which receiver each belongs to. (This comment used to claim it was never
    /// called; that was wrong, and being wrong about it is what made the task's answer
    /// unverifiable — review #15.)
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
