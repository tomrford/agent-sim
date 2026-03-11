use crate::daemon::lifecycle::socket_path;
use crate::ipc::{self, BoxedLocalStream};
use crate::protocol::{
    InstanceAction, Request, RequestAction, Response, ResponseData, WorkerAction,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf, split};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

enum InstanceWorkerMessage {
    Request {
        action: RequestAction,
        response_tx: oneshot::Sender<Result<ResponseData, String>>,
    },
}

pub(super) struct InstanceWorker {
    request_tx: mpsc::Sender<InstanceWorkerMessage>,
}

impl InstanceWorker {
    pub(super) async fn connect(instance_name: &str) -> Result<Self, String> {
        let stream = ipc::connect(&socket_path(instance_name))
            .await
            .map_err(|err| {
                format!(
                    "failed to connect env worker to instance '{}': {err}",
                    instance_name
                )
            })?;
        let (read_half, write_half) = split(stream);
        let (request_tx, request_rx) = mpsc::channel(64);
        tokio::spawn(run_worker(
            instance_name.to_string(),
            read_half,
            write_half,
            request_rx,
        ));
        Ok(Self { request_tx })
    }

    pub(super) async fn begin_instance_request(
        &self,
        action: InstanceAction,
    ) -> Result<oneshot::Receiver<Result<ResponseData, String>>, String> {
        self.begin_request(RequestAction::Instance(action)).await
    }

    pub(super) async fn begin_worker_request(
        &self,
        action: WorkerAction,
    ) -> Result<oneshot::Receiver<Result<ResponseData, String>>, String> {
        self.begin_request(RequestAction::Worker(action)).await
    }

    async fn begin_request(
        &self,
        action: RequestAction,
    ) -> Result<oneshot::Receiver<Result<ResponseData, String>>, String> {
        let (response_tx, response_rx) = oneshot::channel();
        self.request_tx
            .send(InstanceWorkerMessage::Request {
                action,
                response_tx,
            })
            .await
            .map_err(|_| "instance worker request channel closed".to_string())?;
        Ok(response_rx)
    }
}

async fn run_worker(
    instance_name: String,
    read_half: ReadHalf<BoxedLocalStream>,
    mut write_half: WriteHalf<BoxedLocalStream>,
    mut request_rx: mpsc::Receiver<InstanceWorkerMessage>,
) {
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    while let Some(message) = request_rx.recv().await {
        let InstanceWorkerMessage::Request {
            action,
            response_tx,
        } = message;
        let request = Request {
            id: Uuid::new_v4(),
            action,
        };
        let result = send_request(
            &instance_name,
            &mut reader,
            &mut write_half,
            &request,
            &mut line,
        )
        .await;
        let _ = response_tx.send(result);
    }
}

async fn send_request(
    instance_name: &str,
    reader: &mut BufReader<ReadHalf<BoxedLocalStream>>,
    write_half: &mut WriteHalf<BoxedLocalStream>,
    request: &Request,
    line: &mut String,
) -> Result<ResponseData, String> {
    let mut payload = serde_json::to_string(request)
        .map_err(|err| format!("failed to serialize worker request: {err}"))?;
    payload.push('\n');
    write_half
        .write_all(payload.as_bytes())
        .await
        .map_err(|err| format!("failed writing env worker request to '{instance_name}': {err}"))?;

    line.clear();
    let read = reader.read_line(line).await.map_err(|err| {
        format!("failed reading env worker response from '{instance_name}': {err}")
    })?;
    if read == 0 {
        return Err(format!(
            "instance '{}' closed its worker connection unexpectedly",
            instance_name
        ));
    }
    let response: Response = serde_json::from_str(line.trim_end())
        .map_err(|err| format!("invalid env worker response from '{instance_name}': {err}"))?;
    if response.success {
        response
            .data
            .ok_or_else(|| format!("missing response payload from instance '{instance_name}'"))
    } else {
        Err(response
            .error
            .unwrap_or_else(|| format!("instance '{instance_name}' request failed")))
    }
}
