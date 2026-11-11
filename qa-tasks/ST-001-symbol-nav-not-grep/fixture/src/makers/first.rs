use crate::{Run, Worker};

pub fn make(id: u32) -> impl Run {
    Worker { id }
}
