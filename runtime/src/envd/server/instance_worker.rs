use crate::connection;
use crate::protocol::{InstanceAction, Request, RequestAction, ResponseData, WorkerAction};
use tokio::sync::oneshot;
use uuid::Uuid;

pub(super) struct InstanceWorker {
    instance_name: String,
}

impl InstanceWorker {
    pub(super) async fn connect(instance_name: &str) -> Result<Self, String> {
        Ok(Self {
            instance_name: instance_name.to_string(),
        })
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
        let instance_name = self.instance_name.clone();
        tokio::spawn(async move {
            let request = Request {
                id: Uuid::new_v4(),
                action,
            };
            let result = send_request(&instance_name, &request).await;
            let _ = response_tx.send(result);
        });
        Ok(response_rx)
    }
}

async fn send_request(instance_name: &str, request: &Request) -> Result<ResponseData, String> {
    let response = connection::send_request(instance_name, request)
        .await
        .map_err(|err| format!("worker request to '{instance_name}' failed: {err}"))?;
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
