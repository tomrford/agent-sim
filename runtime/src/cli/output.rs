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
        }) => {
            println!("Loaded: {libpath}");
            println!("Signals: {signal_count}");
        }
        Some(ResponseData::ProjectInfo {
            libpath,
            tick_duration_us,
            signal_count,
        }) => {
            println!("Loaded: true");
            println!("Project: {libpath}");
            println!("Tick(us): {tick_duration_us}");
            println!("Signals: {signal_count}");
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
        Some(ResponseData::SignalValues { values }) => {
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
        Some(ResponseData::SetResult { writes_applied }) => {
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
        Some(ResponseData::CanBuses { buses }) => {
            let mut table = Table::new();
            table.load_preset(UTF8_HORIZONTAL_ONLY).set_header(vec![
                "ID",
                "Bus",
                "Bitrate",
                "Data Bitrate",
                "FD",
                "Attached",
            ]);
            for bus in buses {
                table.add_row(vec![
                    bus.id.to_string(),
                    bus.name.clone(),
                    bus.bitrate.to_string(),
                    if bus.bitrate_data == 0 {
                        "-".to_string()
                    } else {
                        bus.bitrate_data.to_string()
                    },
                    bus.fd_capable.to_string(),
                    bus.attached_iface
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ]);
            }
            println!("{table}");
        }
        Some(ResponseData::CanSend { bus, arb_id, len }) => {
            println!("Sent frame on {bus}: id=0x{arb_id:X} len={len}");
        }
        Some(ResponseData::DbcLoaded { bus, signal_count }) => {
            println!("Loaded DBC for {bus}: {signal_count} signals");
        }
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
            env,
        }) => {
            println!("Session: {session}");
            println!("Socket: {socket_path}");
            println!("Running: {running}");
            println!("Env: {}", env.clone().unwrap_or_else(|| "-".to_string()));
        }
        Some(ResponseData::SessionList { sessions }) => {
            let mut table = Table::new();
            table
                .load_preset(UTF8_HORIZONTAL_ONLY)
                .set_header(vec!["Session", "Running", "Env", "Socket"]);
            for item in sessions {
                table.add_row(vec![
                    item.name.clone(),
                    item.running.to_string(),
                    item.env.clone().unwrap_or_else(|| "-".to_string()),
                    item.socket_path.clone(),
                ]);
            }
            println!("{table}");
        }
        None => println!("ok"),
    }
}
