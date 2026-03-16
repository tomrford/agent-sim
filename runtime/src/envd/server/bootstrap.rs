use super::{EnvCanBusState, EnvState};
use crate::can::CanSocket;
use crate::can::dbc::DbcBusOverlay;
use crate::daemon::lifecycle::{bootstrap_daemon_with_exe, kill_pid, read_pid};
use crate::envd::spec::{EnvInstanceSpec, EnvSpec};
use crate::protocol::{CanBusData, InstanceAction, ResponseData, SignalData, WorkerAction};
use crate::signal_selectors::{EnvSignalCatalog, EnvSignalCatalogEntry};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

fn validate_env_tick_duration_us(
    instance_name: &str,
    tick_duration_us: u32,
) -> Result<u32, String> {
    if tick_duration_us == 0 {
        return Err(format!(
            "instance '{instance_name}' reports invalid zero tick duration"
        ));
    }
    Ok(tick_duration_us)
}

impl EnvState {
    pub(super) async fn bootstrap(socket_path: PathBuf, env_spec: EnvSpec) -> Result<Self, String> {
        tracing::info!(
            "bootstrapping env '{}' with {} instance(s)",
            env_spec.name,
            env_spec.instances.len()
        );
        let mut started_instances = Vec::new();
        let result = Self::bootstrap_inner(socket_path, &env_spec, &mut started_instances).await;
        if result.is_err() {
            rollback_instances(&started_instances).await;
        }
        result
    }

    async fn bootstrap_inner(
        socket_path: PathBuf,
        env_spec: &EnvSpec,
        started_instances: &mut Vec<String>,
    ) -> Result<Self, String> {
        tracing::info!("env '{}' phase: bootstrap instances", env_spec.name);
        let mut tick_duration_us = None;
        let mut instance_can_buses: HashMap<String, Vec<CanBusData>> = HashMap::new();
        let mut instance_signals: HashMap<String, Vec<SignalData>> = HashMap::new();
        let mut bootstrap_tasks = tokio::task::JoinSet::new();
        for instance in &env_spec.instances {
            let instance = instance.clone();
            bootstrap_tasks.spawn(async move {
                bootstrap_instance_detached(&instance)
                    .await
                    .map(|_| instance.name)
            });
        }
        while let Some(result) = bootstrap_tasks.join_next().await {
            let instance_name = result.map_err(|err| err.to_string())??;
            started_instances.push(instance_name);
        }

        tracing::info!("env '{}' phase: connect instance workers", env_spec.name);
        let mut instance_workers = HashMap::with_capacity(env_spec.instances.len());
        for instance in &env_spec.instances {
            instance_workers.insert(
                instance.name.clone(),
                super::instance_worker::InstanceWorker::connect(&instance.name).await?,
            );
        }

        tracing::info!("env '{}' phase: fetch instance info", env_spec.name);
        let mut pending_info = Vec::with_capacity(env_spec.instances.len());
        for instance in &env_spec.instances {
            let worker = instance_workers
                .get(&instance.name)
                .ok_or_else(|| format!("missing env worker for instance '{}'", instance.name))?;
            let response_rx = worker.begin_instance_request(InstanceAction::Info).await?;
            pending_info.push((instance.name.clone(), response_rx));
        }
        for (instance_name, response_rx) in pending_info {
            let response = response_rx.await.map_err(|_| {
                format!("info response channel closed for instance '{instance_name}'")
            })??;
            let ResponseData::ProjectInfo {
                tick_duration_us: instance_tick_us,
                ..
            } = response
            else {
                return Err(format!(
                    "unexpected info payload while bootstrapping instance '{}'",
                    instance_name
                ));
            };
            let instance_tick_us = validate_env_tick_duration_us(&instance_name, instance_tick_us)?;
            match tick_duration_us {
                Some(expected) if expected != instance_tick_us => {
                    return Err(format!(
                        "env '{}' requires matching tick durations, but instance '{}' reports {}us and another member reports {}us",
                        env_spec.name, instance_name, instance_tick_us, expected
                    ));
                }
                None => tick_duration_us = Some(instance_tick_us),
                _ => {}
            }
        }

        tracing::info!("env '{}' phase: fetch instance signal catalogs", env_spec.name);
        let mut pending_signals = Vec::with_capacity(env_spec.instances.len());
        for instance in &env_spec.instances {
            let worker = instance_workers
                .get(&instance.name)
                .ok_or_else(|| format!("missing env worker for instance '{}'", instance.name))?;
            let response_rx = worker.begin_instance_request(InstanceAction::Signals).await?;
            pending_signals.push((instance.name.clone(), response_rx));
        }
        for (instance_name, response_rx) in pending_signals {
            let response = response_rx.await.map_err(|_| {
                format!("signal-catalog response channel closed for instance '{instance_name}'")
            })??;
            let ResponseData::Signals { signals } = response else {
                return Err(format!(
                    "unexpected signal-catalog payload while bootstrapping instance '{}'",
                    instance_name
                ));
            };
            instance_signals.insert(instance_name, signals);
        }

        let signal_catalog = build_env_signal_catalog(&instance_signals)?;

        tracing::info!("env '{}' phase: fetch worker can buses", env_spec.name);
        let mut pending_can_buses = Vec::with_capacity(env_spec.instances.len());
        for instance in &env_spec.instances {
            let worker = instance_workers
                .get(&instance.name)
                .ok_or_else(|| format!("missing env worker for instance '{}'", instance.name))?;
            let response_rx = worker.begin_worker_request(WorkerAction::CanBuses).await?;
            pending_can_buses.push((instance.name.clone(), response_rx));
        }
        for (instance_name, response_rx) in pending_can_buses {
            let response = response_rx.await.map_err(|_| {
                format!("CAN bus response channel closed for instance '{instance_name}'")
            })??;
            let ResponseData::CanBuses { buses } = response else {
                return Err(format!(
                    "unexpected CAN bus payload while bootstrapping instance '{}'",
                    instance_name
                ));
            };
            instance_can_buses.insert(instance_name, buses);
        }

        tracing::info!("env '{}' phase: build env can buses", env_spec.name);
        let mut can_buses = BTreeMap::new();
        let mut attached_members = HashSet::new();
        for bus_spec in &env_spec.can_buses {
            let mut fd_capable = false;
            let mut bitrate = 0_u32;
            let mut bitrate_data = 0_u32;
            for member in &bus_spec.members {
                let member_buses =
                    instance_can_buses
                        .get(&member.instance_name)
                        .ok_or_else(|| {
                            format!(
                                "missing CAN bus registry for instance '{}'",
                                member.instance_name
                            )
                        })?;
                let meta = member_buses
                    .iter()
                    .find(|bus| bus.name == member.bus_name)
                    .ok_or_else(|| {
                        format!(
                            "env CAN bus '{}' references missing bus '{}:{}'",
                            bus_spec.name, member.instance_name, member.bus_name
                        )
                    })?;
                fd_capable |= meta.fd_capable;
                bitrate = bitrate.max(meta.bitrate);
                bitrate_data = bitrate_data.max(meta.bitrate_data);
                if !attached_members.insert((member.instance_name.clone(), member.bus_name.clone()))
                {
                    return Err(format!(
                        "instance '{}:{}' is attached to multiple env CAN buses",
                        member.instance_name, member.bus_name
                    ));
                }
            }

            let socket = CanSocket::open(&bus_spec.vcan_iface, bitrate, bitrate_data, fd_capable)?;
            let dbc = match &bus_spec.dbc_path {
                Some(path) => Some(DbcBusOverlay::load(Path::new(path))?),
                None => None,
            };
            can_buses.insert(
                bus_spec.name.clone(),
                EnvCanBusState {
                    name: bus_spec.name.clone(),
                    vcan_iface: bus_spec.vcan_iface.clone(),
                    fd_capable,
                    bitrate,
                    bitrate_data,
                    socket,
                    dbc,
                    latest_frames: HashMap::new(),
                    schedules: BTreeMap::new(),
                },
            );
        }

        tracing::info!("env '{}' phase: attach instance can buses", env_spec.name);
        for bus_spec in &env_spec.can_buses {
            for member in &bus_spec.members {
                let worker = instance_workers.get(&member.instance_name).ok_or_else(|| {
                    format!("missing env worker for instance '{}'", member.instance_name)
                })?;
                let response_rx = worker
                    .begin_worker_request(WorkerAction::CanAttach {
                        bus_name: member.bus_name.clone(),
                        vcan_iface: bus_spec.vcan_iface.clone(),
                    })
                    .await?;
                let response = response_rx.await.map_err(|_| {
                    format!(
                        "CAN attach response channel closed for instance '{}'",
                        member.instance_name
                    )
                })??;
                if !matches!(response, ResponseData::Ack) {
                    return Err(format!(
                        "unexpected CAN attach payload while bootstrapping instance '{}'",
                        member.instance_name
                    ));
                }
            }
        }

        tracing::info!("env '{}' phase: attach shared channels", env_spec.name);
        for shared in &env_spec.shared_channels {
            attach_shared_channel(&env_spec.name, shared).await?;
        }

        let tick_duration_us = tick_duration_us.ok_or_else(|| {
            format!(
                "env '{}' must define at least one instance with a valid tick duration",
                env_spec.name
            )
        })?;

        tracing::info!("env '{}' phase: bootstrap complete", env_spec.name);
        Ok(Self {
            name: env_spec.name.clone(),
            socket_path,
            tick_duration_us,
            instances: env_spec
                .instances
                .iter()
                .map(|instance| instance.name.clone())
                .collect(),
            signal_catalog,
            instance_workers,
            time: crate::sim::time::TimeEngine::default(),
            realtime_tick_backlog: 0,
            can_buses,
            shutdown: false,
        })
    }
}

fn build_env_signal_catalog(
    instance_signals: &HashMap<String, Vec<SignalData>>,
) -> Result<EnvSignalCatalog, String> {
    let mut entries = Vec::new();
    for (instance, signals) in instance_signals {
        for signal in signals {
            if signal.name.contains(':') {
                return Err(format!(
                    "instance '{}' exposes signal '{}' with reserved ':'",
                    instance, signal.name
                ));
            }
            entries.push(EnvSignalCatalogEntry {
                instance: instance.clone(),
                local_id: signal.id,
                signal_name: signal.name.clone(),
                qualified_name: format!("{}:{}", instance, signal.name),
                signal_type: signal.signal_type,
                units: signal.units.clone(),
            });
        }
    }
    EnvSignalCatalog::build(entries)
}

pub(super) async fn attach_shared_channel(
    env_name: &str,
    shared: &crate::envd::spec::EnvSharedChannelSpec,
) -> Result<(), String> {
    let shared_root = crate::daemon::lifecycle::session_root().join("shared");
    std::fs::create_dir_all(&shared_root).map_err(|err| {
        format!(
            "failed to create shared region root '{}': {err}",
            shared_root.display()
        )
    })?;
    let region_path = shared_root.join(format!("{env_name}__{}.bin", shared.name));
    let region_path_str = region_path.display().to_string();

    if !shared
        .members
        .iter()
        .any(|member| member.instance_name == shared.writer_instance)
    {
        return Err(format!(
            "shared channel '{}' writer '{}' is not listed in members",
            shared.name, shared.writer_instance
        ));
    }

    let mut members = shared.members.iter().collect::<Vec<_>>();
    members.sort_by_key(|member| member.instance_name != shared.writer_instance);

    for member in members {
        send_action_success(
            &member.instance_name,
            InstanceAction::SharedAttach {
                channel_name: member.channel_name.clone(),
                path: region_path_str.clone(),
                writer: member.instance_name == shared.writer_instance,
                writer_session: shared.writer_instance.clone(),
            },
        )
        .await?;
    }
    Ok(())
}

async fn send_action_success(instance: &str, action: InstanceAction) -> Result<(), String> {
    let response = crate::connection::send_request(
        instance,
        &crate::protocol::Request {
            id: uuid::Uuid::new_v4(),
            action: crate::protocol::RequestAction::Instance(action),
        },
    )
    .await
    .map_err(|err| err.to_string())?;
    if response.success {
        Ok(())
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "command failed".to_string()))
    }
}

async fn bootstrap_instance_detached(instance: &EnvInstanceSpec) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    bootstrap_instance_detached_with_exe(instance, &exe).await
}

pub(super) async fn bootstrap_instance_detached_with_exe(
    instance: &EnvInstanceSpec,
    exe: &std::path::Path,
) -> Result<(), String> {
    bootstrap_daemon_with_exe(&instance.name, &instance.load_spec, exe)
        .await
        .map_err(|err| format!("failed to bootstrap instance '{}': {err}", instance.name))
}

async fn rollback_instances(started_instances: &[String]) {
    shutdown_bootstrapped_instances(started_instances).await;
}

async fn shutdown_bootstrapped_instances(instances: &[String]) {
    for instance in instances {
        if send_action_success(instance, InstanceAction::Close)
            .await
            .is_err()
            && let Some(pid) = read_pid(instance)
        {
            let _ = kill_pid(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate_env_tick_duration_us;

    #[test]
    fn validate_env_tick_duration_rejects_zero() {
        let err = validate_env_tick_duration_us("demo", 0).expect_err("zero tick must fail");
        assert!(err.contains("invalid zero tick duration"));
    }

    #[test]
    fn validate_env_tick_duration_accepts_positive_value() {
        assert_eq!(
            validate_env_tick_duration_us("demo", 50).expect("positive tick must pass"),
            50
        );
    }
}
