use super::{EnvState, observe_env_bus_frames, record_env_frame};
use crate::protocol::{ResponseData, WorkerAction};
use tokio::task::yield_now;

pub(super) async fn advance_due_ticks(state: &mut EnvState, due_ticks: u64) -> Result<(), String> {
    for _ in 0..due_ticks {
        if state.shutdown {
            return Ok(());
        }
        advance_single_tick(state).await?;
        yield_now().await;
    }
    Ok(())
}

pub(super) async fn advance_single_tick(state: &mut EnvState) -> Result<(), String> {
    let current_tick = state.time.status(state.tick_duration_us).elapsed_ticks;
    observe_env_bus_frames(state)?;

    for bus in state.can_buses.values_mut() {
        let mut emitted_frames = Vec::new();
        for schedule in bus.schedules.values_mut() {
            if !schedule.enabled || schedule.next_due_tick > current_tick {
                continue;
            }
            bus.socket.send(&schedule.frame)?;
            emitted_frames.push(schedule.frame.clone());
            schedule.next_due_tick = current_tick.saturating_add(schedule.every_ticks.max(1));
        }
        for frame in emitted_frames {
            record_env_frame(bus, &frame);
        }
    }

    let instances = state.instances.clone();
    let mut pending = Vec::with_capacity(instances.len());
    for instance in &instances {
        let worker = state
            .instance_workers
            .get(instance)
            .ok_or_else(|| format!("missing env worker for instance '{instance}'"))?;
        let response_rx = worker.begin_worker_request(WorkerAction::Step).await?;
        pending.push((instance.clone(), response_rx));
    }

    for (instance, response_rx) in pending {
        let response = response_rx.await.map_err(|_| {
            format!("worker-step response channel closed for instance '{instance}'")
        })??;
        if !matches!(response, ResponseData::Ack) {
            return Err(format!(
                "unexpected worker-step payload while stepping instance '{instance}'"
            ));
        }
    }
    observe_env_bus_frames(state)?;
    let sample_tick = current_tick.saturating_add(1);
    super::dispatch::sample_env_trace_after_tick(state, sample_tick).await?;

    state.time.advance_ticks(1);
    Ok(())
}
