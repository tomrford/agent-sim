use super::action_router::advance_single_project_tick;
use super::{ActionMessage, DaemonState, process_action_message};
use tokio::sync::{mpsc, watch};
use tokio::task::yield_now;
use tokio::time::sleep;

const MAX_NORMAL_ACTIONS_PER_TURN: usize = 16;
const MAX_WORKER_ACTIONS_PER_TURN: usize = 64;
const MAX_REALTIME_TICKS_PER_TURN: u64 = 64;

pub(super) async fn run_tick_task(
    mut state: DaemonState,
    mut action_rx: mpsc::Receiver<ActionMessage>,
    mut worker_action_rx: mpsc::Receiver<ActionMessage>,
    shutdown_tx: watch::Sender<bool>,
) {
    loop {
        process_action_batch(
            &mut worker_action_rx,
            &mut state,
            MAX_WORKER_ACTIONS_PER_TURN,
        )
        .await;
        process_action_batch(&mut action_rx, &mut state, MAX_NORMAL_ACTIONS_PER_TURN).await;

        if state.shutdown {
            break;
        }

        state.realtime_tick_backlog = state.realtime_tick_backlog.saturating_add(
            state
                .time
                .tick_realtime_due(state.project.tick_duration_us()),
        );
        let tick_batch = state.realtime_tick_backlog.min(MAX_REALTIME_TICKS_PER_TURN);
        if let Err(err) = advance_project_ticks(&mut state, tick_batch) {
            tracing::error!("tick task failed for session '{}': {err}", state.session);
            state.shutdown = true;
        } else {
            state.realtime_tick_backlog = state.realtime_tick_backlog.saturating_sub(tick_batch);
        }

        if state.shutdown {
            break;
        }

        if state.realtime_tick_backlog > 0 {
            yield_now().await;
            continue;
        }

        let sleep_duration = state
            .time
            .realtime_poll_delay(state.project.tick_duration_us());
        tokio::select! {
            biased;
            received = worker_action_rx.recv() => match received {
                Some(message) => process_action_message(message, &mut state).await,
                None if action_rx.is_closed() => break,
                None => {}
            },
            received = action_rx.recv() => match received {
                Some(message) => process_action_message(message, &mut state).await,
                None if worker_action_rx.is_closed() => break,
                None => {}
            },
            _ = sleep(sleep_duration) => {}
        }
    }

    let _ = shutdown_tx.send(true);
}

async fn process_action_batch(
    action_rx: &mut mpsc::Receiver<ActionMessage>,
    state: &mut DaemonState,
    max_actions: usize,
) {
    for _ in 0..max_actions {
        let Ok(message) = action_rx.try_recv() else {
            break;
        };
        process_action_message(message, state).await;
        if state.shutdown {
            break;
        }
    }
}

fn advance_project_ticks(state: &mut DaemonState, ticks: u64) -> Result<(), String> {
    let mut processed = 0_u64;
    for _ in 0..ticks {
        if let Err(err) = advance_single_project_tick(state) {
            state.time.advance_ticks(if err.advance_tick() {
                processed.saturating_add(1)
            } else {
                processed
            });
            return Err(err.into_message());
        }
        processed = processed.saturating_add(1);
    }
    state.time.advance_ticks(processed);
    Ok(())
}
