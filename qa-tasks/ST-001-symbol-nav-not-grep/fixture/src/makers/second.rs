use crate::{Backup, Run};

pub fn make(id: u32) -> impl Run {
    Backup { id }
}
