use crate::protocol::{Response, ResponseData};
use comfy_table::{ContentArrangement, Table, presets::UTF8_HORIZONTAL_ONLY};
use serde_json::json;

pub fn print_response(response: &Response, json_mode: bool) {
    if json_mode {
        if let Some(ResponseData::WatchSamples { samples }) = &response.data {
            for sample in samples {
                let line = json!({
                    "tick": sample.tick,
                    "time_us": sample.time_us,
                    "instance": sample.instance,
                    "name": sample.signal,
                    "value": sample.value
                });
                println!(
                    "{}",
                    serde_json::to_string(&line).unwrap_or_else(|_| "{}".to_string())
                );
            }
            return;
        }
        println!(
            "{}",
            serde_json::to_string(response).unwrap_or_else(|_| {
                "{\"success\":false,\"error\":\"failed to serialize response\"}".to_string()
            })
        );
        return;
    }

    if !response.success {
        eprintln!("{}", response.error.as_deref().unwrap_or("unknown error"));
        return;
    }
    match &response.data {
        Some(ResponseData::Ack) => println!("ok"),
        Some(ResponseData::Loaded {
            libpath,
            signal_count,
            instance_count,
        }) => {
            println!("Loaded: {libpath}");
            println!("Signals: {signal_count}");
            println!("Instances: {instance_count}");
        }
        Some(ResponseData::ProjectInfo {
            loaded,
            libpath,
            tick_duration_us,
            signal_count,
            instance_count,
            active_instance,
        }) => {
            println!("Loaded: {loaded}");
            if let Some(path) = libpath {
                println!("Project: {path}");
            }
            if let Some(tick) = tick_duration_us {
                println!("Tick(us): {tick}");
            }
            println!("Signals: {signal_count}");
            println!("Instances: {instance_count}");
            println!(
                "Active instance: {}",
                active_instance
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
        }
        Some(ResponseData::Signals { signals }) => {
            let mut table = Table::new();
            table
                .load_preset(UTF8_HORIZONTAL_ONLY)
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_header(vec!["ID", "Name", "Type", "Units"]);
            for signal in signals {
                table.add_row(vec![
                    signal.id.to_string(),
                    signal.name.clone(),
                    signal.signal_type.to_string(),
                    signal.units.clone().unwrap_or_else(|| "-".to_string()),
                ]);
            }
            println!("{table}");
        }
        Some(ResponseData::Instances {
            instances,
            active_instance,
        }) => {
            let mut table = Table::new();
            table
                .load_preset(UTF8_HORIZONTAL_ONLY)
                .set_header(vec!["Index", "Active"]);
            for instance in instances {
                table.add_row(vec![
                    instance.index.to_string(),
                    if Some(instance.index) == *active_instance {
                        "yes".to_string()
                    } else {
                        String::new()
                    },
                ]);
            }
            println!("{table}");
        }
        Some(ResponseData::SelectedInstance { active_instance }) => {
            println!("Active instance: {active_instance}");
        }
        Some(ResponseData::SignalValues { values, .. }) => {
            let mut table = Table::new();
            table
                .load_preset(UTF8_HORIZONTAL_ONLY)
                .set_header(vec!["ID", "Name", "Type", "Value", "Units"]);
            for signal in values {
                table.add_row(vec![
                    signal.id.to_string(),
                    signal.name.clone(),
                    signal.signal_type.to_string(),
                    format!("{:?}", signal.value),
                    signal.units.clone().unwrap_or_else(|| "-".to_string()),
                ]);
            }
            println!("{table}");
        }
        Some(ResponseData::SetResult {
            instance,
            writes_applied,
        }) => {
            println!("Instance: {instance}");
            println!("Writes applied: {writes_applied}");
        }
        Some(ResponseData::TimeStatus {
            state,
            elapsed_ticks,
            elapsed_time_us,
            speed,
        }) => {
            println!(
                "State: {:?}  Ticks: {}  Sim-time: {:.6}s  Speed: {}x",
                state,
                elapsed_ticks,
                *elapsed_time_us as f64 / 1_000_000.0,
                speed
            );
        }
        Some(ResponseData::TimeAdvanced {
            requested_us,
            advanced_ticks,
            advanced_us,
        }) => {
            println!(
                "Requested: {}us  Advanced: {} ticks ({}us)",
                requested_us, advanced_ticks, advanced_us
            );
        }
        Some(ResponseData::Speed { speed }) => println!("{speed}"),
        Some(ResponseData::WatchSamples { samples }) => {
            for sample in samples {
                println!(
                    "tick={} time_us={} {}={:?}",
                    sample.tick, sample.time_us, sample.signal, sample.value
                );
            }
        }
        Some(ResponseData::RecipeResult {
            recipe,
            dry_run,
            steps_executed,
            events,
        }) => {
            println!("Recipe: {recipe}");
            println!("Dry run: {dry_run}");
            println!("Steps: {steps_executed}");
            for event in events {
                println!("- {event}");
            }
        }
        Some(ResponseData::SessionStatus {
            session,
            socket_path,
            running,
        }) => {
            println!("Session: {session}");
            println!("Socket: {socket_path}");
            println!("Running: {running}");
        }
        Some(ResponseData::SessionList { sessions }) => {
            let mut table = Table::new();
            table
                .load_preset(UTF8_HORIZONTAL_ONLY)
                .set_header(vec!["Session", "Running", "Socket"]);
            for item in sessions {
                table.add_row(vec![
                    item.name.clone(),
                    item.running.to_string(),
                    item.socket_path.clone(),
                ]);
            }
            println!("{table}");
        }
        None => println!("ok"),
    }
}
