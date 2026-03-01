use crate::sim::types::SimCtx;

#[derive(Debug)]
pub struct Instance {
    pub index: u32,
    pub ctx: *mut SimCtx,
}

unsafe impl Send for Instance {}
unsafe impl Sync for Instance {}
