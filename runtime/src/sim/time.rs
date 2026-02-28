use crate::protocol::TimeStateData;
use crate::sim::error::{SimError, TimeError};
use crate::sim::instance_manager::InstanceManager;
use crate::sim::project::Project;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct TimeStatus {
    pub state: TimeStateData,
    pub elapsed_ticks: u64,
    pub elapsed_time_us: u64,
    pub speed: f64,
}

#[derive(Debug, Clone)]
pub struct StepResult {
    pub requested_us: u64,
    pub advanced_ticks: u64,
    pub advanced_us: u64,
}

#[derive(Debug)]
pub struct TimeEngine {
    state: TimeStateData,
    speed: f64,
    elapsed_ticks: u64,
    remainder_us: f64,
    last_wallclock: Option<Instant>,
}

impl Default for TimeEngine {
    fn default() -> Self {
        Self {
            state: TimeStateData::Paused,
            speed: 1.0,
            elapsed_ticks: 0,
            remainder_us: 0.0,
            last_wallclock: None,
        }
    }
}

impl TimeEngine {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn status(&self, tick_duration_us: Option<u32>) -> TimeStatus {
        let tick_us = tick_duration_us.unwrap_or(0) as u64;
        TimeStatus {
            state: self.state,
            elapsed_ticks: self.elapsed_ticks,
            elapsed_time_us: self.elapsed_ticks.saturating_mul(tick_us),
            speed: self.speed,
        }
    }

    pub fn start(&mut self) -> Result<(), TimeError> {
        if self.state == TimeStateData::Running {
            return Err(TimeError::AlreadyRunning);
        }
        self.state = TimeStateData::Running;
        self.last_wallclock = Some(Instant::now());
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), TimeError> {
        if self.state == TimeStateData::Paused {
            return Err(TimeError::AlreadyPaused);
        }
        self.state = TimeStateData::Paused;
        self.last_wallclock = None;
        self.remainder_us = 0.0;
        Ok(())
    }

    pub fn set_speed(&mut self, speed: f64) -> Result<(), TimeError> {
        if !speed.is_finite() || speed <= 0.0 {
            return Err(TimeError::InvalidSpeed(speed));
        }
        self.speed = speed;
        Ok(())
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn step(
        &mut self,
        project: &Project,
        instances: &InstanceManager,
        duration_us: u64,
    ) -> Result<StepResult, TimeError> {
        if self.state == TimeStateData::Running {
            return Err(TimeError::StepWhileRunning);
        }
        let tick_us = project.tick_duration_us() as u64;
        let ticks = if tick_us == 0 { 0 } else { duration_us / tick_us };
        self.tick_all(project, instances, ticks)
            .map_err(|_| TimeError::ProjectNotLoaded)?;
        let advanced_us = ticks.saturating_mul(tick_us);
        Ok(StepResult {
            requested_us: duration_us,
            advanced_ticks: ticks,
            advanced_us,
        })
    }

    pub fn tick_realtime(
        &mut self,
        project: &Project,
        instances: &InstanceManager,
    ) -> Result<u64, SimError> {
        if self.state != TimeStateData::Running {
            return Ok(0);
        }
        let now = Instant::now();
        let Some(last) = self.last_wallclock else {
            self.last_wallclock = Some(now);
            return Ok(0);
        };
        let delta = now.duration_since(last).as_secs_f64() * 1_000_000.0 * self.speed;
        self.last_wallclock = Some(now);
        self.remainder_us += delta;

        let tick_us = project.tick_duration_us() as f64;
        if tick_us <= 0.0 {
            return Ok(0);
        }
        let ticks = (self.remainder_us / tick_us).floor() as u64;
        if ticks == 0 {
            return Ok(0);
        }
        self.remainder_us -= ticks as f64 * tick_us;
        self.tick_all(project, instances, ticks)?;
        Ok(ticks)
    }

    fn tick_all(
        &mut self,
        project: &Project,
        instances: &InstanceManager,
        ticks: u64,
    ) -> Result<(), SimError> {
        for _ in 0..ticks {
            for ctx in instances.iter_ctxs() {
                project.tick_ctx(ctx)?;
            }
            self.elapsed_ticks = self.elapsed_ticks.saturating_add(1);
        }
        Ok(())
    }
}
