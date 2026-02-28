use crate::sim::error::InstanceError;
use crate::sim::instance::Instance;
use crate::sim::project::Project;
use crate::sim::types::SimCtx;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct InstanceManager {
    instances: BTreeMap<u32, Instance>,
    next_index: u32,
    active_index: Option<u32>,
}

impl InstanceManager {
    pub fn clear(&mut self, project: &Project) {
        let instances = std::mem::take(&mut self.instances);
        for (_, instance) in instances {
            project.free_ctx(instance.ctx);
        }
        self.next_index = 0;
        self.active_index = None;
    }

    pub fn create(&mut self, project: &Project) -> Result<u32, crate::sim::error::SimError> {
        let ctx = project.new_ctx()?;
        let index = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        self.instances.insert(index, Instance { index, ctx });
        if self.active_index.is_none() {
            self.active_index = Some(index);
        }
        Ok(index)
    }

    pub fn list(&self) -> Vec<u32> {
        self.instances.keys().copied().collect()
    }

    pub fn select(&mut self, index: u32) -> Result<(), InstanceError> {
        if !self.instances.contains_key(&index) {
            return Err(InstanceError::IndexOutOfRange(index));
        }
        self.active_index = Some(index);
        Ok(())
    }

    pub fn active_index(&self) -> Option<u32> {
        self.active_index
    }

    pub fn resolve_target(&self, override_index: Option<u32>) -> Result<u32, InstanceError> {
        let index = override_index
            .or(self.active_index)
            .ok_or(InstanceError::NoActiveInstance)?;
        if !self.instances.contains_key(&index) {
            return Err(InstanceError::IndexOutOfRange(index));
        }
        Ok(index)
    }

    pub fn get_ctx(&self, index: u32) -> Result<*mut SimCtx, InstanceError> {
        self.instances
            .get(&index)
            .map(|v| v.ctx)
            .ok_or(InstanceError::IndexOutOfRange(index))
    }

    pub fn reset(&self, project: &Project, index: u32) -> Result<(), crate::sim::error::SimError> {
        let ctx = self
            .get_ctx(index)
            .map_err(|_| crate::sim::error::SimError::InvalidCtx)?;
        project.reset_ctx(ctx)
    }

    pub fn free(&mut self, project: &Project, index: u32) -> Result<(), InstanceError> {
        let instance = self
            .instances
            .remove(&index)
            .ok_or(InstanceError::IndexOutOfRange(index))?;
        project.free_ctx(instance.ctx);
        if self.active_index == Some(index) {
            self.active_index = self.instances.keys().next().copied();
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn iter_ctxs(&self) -> impl Iterator<Item = *mut SimCtx> + '_ {
        self.instances.values().map(|instance| instance.ctx)
    }
}
