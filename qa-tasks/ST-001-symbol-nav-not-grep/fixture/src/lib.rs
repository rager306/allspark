//! Two types with a same-named method, and the two callers that invoke them.

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

/// The method both callers invoke. Its return type is `impl Run`, never the concrete
/// struct, so no signature names the receiver's type.
pub trait Run {
    fn run(&self) -> u32;
}

impl Run for Worker {
    fn run(&self) -> u32 {
        Worker::run(self)
    }
}

impl Run for Backup {
    fn run(&self) -> u32 {
        Backup::run(self)
    }
}

mod makers;
mod spawn;
mod unrelated;

pub use spawn::spawn_worker;
pub use unrelated::unrelated;
