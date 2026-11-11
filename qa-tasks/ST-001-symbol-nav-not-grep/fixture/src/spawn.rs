use crate::makers::first;
use crate::Run;

pub fn spawn_worker(id: u32) -> u32 {
    let x = first::make(id);
    x.run()
}
