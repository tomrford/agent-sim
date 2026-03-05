use super::{ActionMessage, DaemonState, process_action_message};
use super::{can_ops, shared_ops};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;

pub(super) async fn run_tick_task(
    mut state: DaemonState,
    mut action_rx: mpsc::Receiver<ActionMessage>,
    shutdown_tx: watch::Sender<bool>,
) {
    loop {
        while let Ok(message) = action_rx.try_recv() {
            process_action_message(message, &mut state).await;
        }

        if state.shutdown {
            break;
        }

        let due_ticks = state
            .time
            .tick_realtime_due(state.project.tick_duration_us());
        if let Err(err) = advance_project_ticks(&mut state, due_ticks) {
            tracing::error!("tick task failed for session '{}': {err}", state.session);
            state.shutdown = true;
        }

        if state.shutdown {
            break;
        }

        let sleep_duration = state
            .time
            .realtime_poll_delay(state.project.tick_duration_us());
        match timeout(sleep_duration, action_rx.recv()).await {
            Ok(Some(message)) => process_action_message(message, &mut state).await,
            Ok(None) => break,
            Err(_) => {}
        }
    }

    let _ = shutdown_tx.send(true);
}

fn advance_project_ticks(state: &mut DaemonState, ticks: u64) -> Result<(), String> {
    let mut processed = 0_u64;
    for _ in 0..ticks {
        if let Err(err) = can_ops::process_can_rx(state) {
            state.time.advance_ticks(processed);
            return Err(err);
        }
        if let Err(err) = shared_ops::process_shared_rx(state) {
            state.time.advance_ticks(processed);
            return Err(err);
        }
        if let Err(err) = state.project.tick() {
            state.time.advance_ticks(processed);
            return Err(err.to_string());
        }
        if let Err(err) = can_ops::process_can_tx(state) {
            state.time.advance_ticks(processed.saturating_add(1));
            return Err(err);
        }
        if let Err(err) = shared_ops::process_shared_tx(state) {
            state.time.advance_ticks(processed.saturating_add(1));
            return Err(err);
        }
        processed = processed.saturating_add(1);
    }
    state.time.advance_ticks(processed);
    Ok(())
}
