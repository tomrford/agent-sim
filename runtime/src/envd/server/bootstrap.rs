use super::{EnvCanBusState, EnvState};
use crate::can::CanSocket;
use crate::can::dbc::DbcBusOverlay;
use crate::daemon::lifecycle::{kill_pid, read_pid};
use crate::envd::spec::{EnvInstanceSpec, EnvSpec};
use crate::load::write_load_spec;
use crate::protocol::{CanBusData, InstanceAction, ResponseData, WorkerAction};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

impl EnvState {
    pub(super) async fn bootstrap(socket_path: PathBuf, env_spec: EnvSpec) -> Result<Self, String> {
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
        let mut tick_duration_us = None;
        let mut instance_can_buses: HashMap<String, Vec<CanBusData>> = HashMap::new();
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

        let mut instance_workers = HashMap::with_capacity(env_spec.instances.len());
        for instance in &env_spec.instances {
            instance_workers.insert(
                instance.name.clone(),
                super::instance_worker::InstanceWorker::connect(&instance.name).await?,
            );
        }

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

        let mut instance_bus_map = HashMap::new();
        let mut can_buses = BTreeMap::new();
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
                if instance_bus_map
                    .insert(
                        (member.instance_name.clone(), member.bus_name.clone()),
                        bus_spec.name.clone(),
                    )
                    .is_some()
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
                    members: bus_spec.members.clone(),
                    socket,
                    dbc,
                    latest_frames: HashMap::new(),
                    pending_delivery: Vec::new(),
                    schedules: BTreeMap::new(),
                },
            );
        }

        for shared in &env_spec.shared_channels {
            attach_shared_channel(&env_spec.name, shared).await?;
        }

        Ok(Self {
            name: env_spec.name.clone(),
            socket_path,
            tick_duration_us: tick_duration_us.unwrap_or(1),
            instances: env_spec
                .instances
                .iter()
                .map(|instance| instance.name.clone())
                .collect(),
            instance_workers,
            time: crate::sim::time::TimeEngine::default(),
            can_buses,
            instance_bus_map,
            ignored_instance_buses: std::collections::HashSet::new(),
            shutdown: false,
        })
    }
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

async fn run_bootstrap_instance_command(
    exe: &Path,
    instance_name: &str,
    spec_path: &Path,
) -> Result<std::process::Output, String> {
    tokio::process::Command::new(exe)
        .arg("__internal")
        .arg("bootstrap-instance")
        .arg("--instance")
        .arg(instance_name)
        .arg("--load-spec-path")
        .arg(spec_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|err| format!("failed to bootstrap instance '{instance_name}': {err}"))
}

async fn bootstrap_instance_detached(instance: &EnvInstanceSpec) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    bootstrap_instance_detached_with_exe(instance, &exe).await
}

pub(super) async fn bootstrap_instance_detached_with_exe(
    instance: &EnvInstanceSpec,
    exe: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(crate::daemon::lifecycle::bootstrap_dir())
        .map_err(|err| format!("failed to create bootstrap dir: {err}"))?;
    let spec_path = crate::daemon::lifecycle::bootstrap_dir().join(format!(
        "{}-helper-{}.json",
        instance.name,
        uuid::Uuid::new_v4()
    ));
    write_load_spec(&spec_path, &instance.load_spec).map_err(|err| err.to_string())?;

    let output = run_bootstrap_instance_command(exe, &instance.name, &spec_path).await;
    let _ = std::fs::remove_file(&spec_path);
    let output = output?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "failed to bootstrap instance '{}': {}",
            instance.name,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
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
