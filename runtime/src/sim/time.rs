use crate::protocol::TimeStateData;
use crate::sim::error::TimeError;
use std::time::Instant;
use tokio::time::Duration;

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

    pub fn status(&self, tick_duration_us: u32) -> TimeStatus {
        let tick_us = tick_duration_us as u64;
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

    pub fn is_running(&self) -> bool {
        self.state == TimeStateData::Running
    }

    pub fn step_ticks(
        &mut self,
        tick_duration_us: u32,
        duration_us: u64,
    ) -> Result<StepResult, TimeError> {
        if self.state == TimeStateData::Running {
            return Err(TimeError::StepWhileRunning);
        }
        let tick_us = tick_duration_us as u64;
        let ticks = if tick_us == 0 {
            0
        } else {
            duration_us / tick_us
        };
        let advanced_us = ticks.saturating_mul(tick_us);
        Ok(StepResult {
            requested_us: duration_us,
            advanced_ticks: ticks,
            advanced_us,
        })
    }

    pub fn tick_realtime_due(&mut self, tick_duration_us: u32) -> u64 {
        if self.state != TimeStateData::Running {
            return 0;
        }
        let now = Instant::now();
        let Some(last) = self.last_wallclock else {
            self.last_wallclock = Some(now);
            return 0;
        };
        let delta = now.duration_since(last).as_secs_f64() * 1_000_000.0 * self.speed;
        self.last_wallclock = Some(now);
        self.remainder_us += delta;

        let tick_us = tick_duration_us as f64;
        if tick_us <= 0.0 {
            return 0;
        }
        let ticks = (self.remainder_us / tick_us).floor() as u64;
        if ticks == 0 {
            return 0;
        }
        self.remainder_us -= ticks as f64 * tick_us;
        ticks
    }

    pub fn realtime_poll_delay(&self, tick_duration_us: u32) -> Duration {
        if self.state != TimeStateData::Running {
            return Duration::from_millis(5);
        }
        if tick_duration_us == 0 {
            return Duration::from_millis(1);
        }
        let tick_us = tick_duration_us as f64;
        let remaining_sim_us = (tick_us - self.remainder_us).max(0.0);
        let wall_us = if self.speed > 0.0 {
            (remaining_sim_us / self.speed).ceil()
        } else {
            tick_us.ceil()
        };
        let clamped_wall_us = wall_us.clamp(100.0, u64::MAX as f64);
        Duration::from_micros(clamped_wall_us as u64)
    }

    pub fn advance_ticks(&mut self, ticks: u64) {
        self.elapsed_ticks = self.elapsed_ticks.saturating_add(ticks);
    }
}

#[cfg(test)]
mod tests {
    use super::TimeEngine;
    use crate::protocol::TimeStateData;
    use tokio::time::Duration;

    #[test]
    fn realtime_poll_delay_scales_with_tick_duration() {
        let mut engine = TimeEngine::default();
        engine.state = TimeStateData::Running;
        engine.speed = 1.0;
        engine.remainder_us = 0.0;
        let delay = engine.realtime_poll_delay(20_000);
        assert!(
            delay >= Duration::from_millis(20),
            "expected >=20ms delay, got {delay:?}"
        );
    }
}
