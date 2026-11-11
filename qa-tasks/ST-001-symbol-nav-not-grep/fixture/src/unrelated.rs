use crate::makers::second;
use crate::Run;

pub fn unrelated(id: u32) -> u32 {
    let x = second::make(id);
    x.run()
}
