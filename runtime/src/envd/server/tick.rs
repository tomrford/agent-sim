use super::{EnvState, PendingFrame, queue_env_frame};
use crate::can::dbc::frame_key_from_frame;
use crate::protocol::{CanBusFramesData, ResponseData, WorkerAction};
use crate::sim::types::SimCanFrame;
use std::collections::HashMap;
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
    let mut instance_rx: HashMap<String, HashMap<String, Vec<SimCanFrame>>> = HashMap::new();
    let current_tick = state.time.status(state.tick_duration_us).elapsed_ticks;

    for bus in state.can_buses.values_mut() {
        let mut ready_frames = std::mem::take(&mut bus.pending_delivery);
        for frame in bus.socket.recv_all()? {
            ready_frames.push(PendingFrame {
                source_instance: None,
                frame,
            });
        }
        for schedule in bus.schedules.values_mut() {
            if !schedule.enabled || schedule.next_due_tick > current_tick {
                continue;
            }
            bus.socket.send(&schedule.frame)?;
            bus.latest_frames.insert(
                frame_key_from_frame(&schedule.frame),
                schedule.frame.clone(),
            );
            ready_frames.push(PendingFrame {
                source_instance: None,
                frame: schedule.frame.clone(),
            });
            schedule.next_due_tick = current_tick.saturating_add(schedule.every_ticks.max(1));
        }

        for pending in ready_frames {
            bus.latest_frames
                .insert(frame_key_from_frame(&pending.frame), pending.frame.clone());
            for member in &bus.members {
                if pending
                    .source_instance
                    .as_ref()
                    .is_some_and(|source| source == &member.instance_name)
                {
                    continue;
                }
                instance_rx
                    .entry(member.instance_name.clone())
                    .or_default()
                    .entry(member.bus_name.clone())
                    .or_default()
                    .push(pending.frame.clone());
            }
        }
    }

    let instances = state.instances.clone();
    let mut pending = Vec::with_capacity(instances.len());
    for instance in &instances {
        let can_rx = instance_rx
            .remove(instance)
            .unwrap_or_default()
            .into_iter()
            .map(|(bus_name, frames)| CanBusFramesData {
                bus_name,
                frames: frames.iter().map(Into::into).collect(),
            })
            .collect::<Vec<_>>();
        let worker = state
            .instance_workers
            .get(instance)
            .ok_or_else(|| format!("missing env worker for instance '{instance}'"))?;
        let response_rx = worker
            .begin_worker_request(WorkerAction::Step { can_rx })
            .await?;
        pending.push((instance.clone(), response_rx));
    }

    for (instance, response_rx) in pending {
        let response = response_rx.await.map_err(|_| {
            format!("worker-step response channel closed for instance '{instance}'")
        })??;
        let ResponseData::WorkerStep { can_tx } = response else {
            return Err(format!(
                "unexpected worker-step payload while stepping instance '{instance}'"
            ));
        };
        for batch in can_tx {
            let Some(env_bus_name) =
                resolve_env_bus_name(state, instance.as_str(), batch.bus_name.as_str())
            else {
                continue;
            };
            for frame in batch
                .frames
                .into_iter()
                .map(SimCanFrame::try_from)
                .collect::<Result<Vec<_>, _>>()?
            {
                queue_env_frame(state, &env_bus_name, Some(instance.clone()), &frame)?;
            }
        }
    }

    state.time.advance_ticks(1);
    Ok(())
}

pub(super) fn resolve_env_bus_name(
    state: &mut EnvState,
    instance: &str,
    bus_name: &str,
) -> Option<String> {
    let key = (instance.to_string(), bus_name.to_string());
    if let Some(env_bus_name) = state.instance_bus_map.get(&key) {
        return Some(env_bus_name.clone());
    }

    if state.ignored_instance_buses.insert(key) {
        tracing::warn!(
            "instance '{}:{}' emitted CAN traffic on an unmapped bus; dropping frames",
            instance,
            bus_name
        );
    }
    None
}
