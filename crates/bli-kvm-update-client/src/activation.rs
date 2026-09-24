use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use bli_kvm_update_protocol::{
    MAX_UPDATE_MESSAGE_BYTES, UPDATE_SCHEMA_VERSION, UpdateProtocolError,
    activation::{
        self, ActivationErrorCode, ActivationReceipt, ActivationRequest, ActivationRequestEnvelope,
        ActivationResponse, PreparedActivationReceipt,
    },
    encode_frame,
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Semaphore,
    time::timeout,
};

#[derive(Debug, Error)]
pub enum ActivationClientError {
    #[error("invalid activation client configuration")]
    Configuration,
    #[error("activation request already in progress")]
    Busy,
    #[error("activation result timed out; reconcile status before any new attempt")]
    Timeout,
    #[error("activation transport failed; reconcile status before any new attempt")]
    Io(#[from] std::io::Error),
    #[error("invalid activation protocol")]
    Protocol(#[from] UpdateProtocolError),
    #[error("activation response does not match request")]
    MismatchedResponse,
    #[error("activation rejected: {0:?}")]
    Rejected(ActivationErrorCode),
}

#[cfg(test)]
mod tests {
    use super::*;
    use activation::ActivationResponseEnvelope;
    use tokio::net::UnixListener;

    fn slot() -> String {
        format!("app-00000000000000000002-{}", "a".repeat(64))
    }

    #[tokio::test]
    async fn prepared_admission_is_generation_bound_distinct_and_requires_eof() {
        for mode in [
            "prepared",
            "legacy-client",
            "wrong-generation",
            "wrong-target",
            "held-eof",
            "legacy-reply",
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("admit.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let worker = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut input = Vec::new();
                stream.read_to_end(&mut input).await.unwrap();
                let request = activation::decode_request(&input[4..]).unwrap();
                assert_eq!(
                    request.request,
                    if mode == "legacy-client" {
                        ActivationRequest::Activate {
                            slot_id: slot(),
                            expected_generation: 41,
                        }
                    } else {
                        ActivationRequest::Admit {
                            slot_id: slot(),
                            expected_generation: 41,
                        }
                    }
                );
                let response = ActivationResponseEnvelope {
                    schema_version: 1,
                    request_id: request.request_id,
                    response: if mode == "legacy-reply" {
                        ActivationResponse::AwaitingHealth {
                            receipt: ActivationReceipt {
                                operation_id: "1".repeat(32),
                                target_slot_id: slot(),
                            },
                        }
                    } else {
                        ActivationResponse::Prepared {
                            receipt: PreparedActivationReceipt {
                                operation_id: "1".repeat(32),
                                target_slot_id: if mode == "wrong-target" {
                                    format!("app-00000000000000000003-{}", "b".repeat(64))
                                } else {
                                    slot()
                                },
                                prepared_generation: if mode == "wrong-generation" {
                                    41
                                } else {
                                    42
                                },
                                deadline_monotonic_ms: u64::MAX,
                            },
                        }
                    },
                };
                stream
                    .write_all(
                        &encode_frame(&activation::encode_response(&response).unwrap()).unwrap(),
                    )
                    .await
                    .unwrap();
                if mode == "held-eof" {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                drop(stream);
                assert!(
                    timeout(Duration::from_millis(20), listener.accept())
                        .await
                        .is_err()
                );
            });
            let mut client = UpdateActivationClient::new(path).unwrap();
            client.deadline = Duration::from_millis(100);
            let result = if mode == "legacy-client" {
                client.activate(&slot(), 41).await.map(|_| ())
            } else {
                client.admit(&slot(), 41).await.map(|receipt| {
                    assert_eq!(receipt.prepared_generation, 42);
                    assert_eq!(receipt.deadline_monotonic_ms, u64::MAX);
                })
            };
            match mode {
                "prepared" => result.unwrap(),
                "held-eof" => assert!(matches!(result, Err(ActivationClientError::Timeout))),
                _ => assert!(matches!(
                    result,
                    Err(ActivationClientError::MismatchedResponse)
                )),
            }
            worker.await.unwrap();
        }
    }

    #[tokio::test]
    async fn exact_once_exchange_checks_id_target_size_eof_and_rejections() {
        for mode in [
            "ok",
            "wrong-id",
            "wrong-target",
            "oversize",
            "trailing",
            "lost-reply",
            "stale",
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("activate.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let worker = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut input = Vec::new();
                stream.read_to_end(&mut input).await.unwrap();
                assert_eq!(
                    u32::from_be_bytes(input[..4].try_into().unwrap()) as usize,
                    input.len() - 4
                );
                let request = activation::decode_request(&input[4..]).unwrap();
                assert_eq!(
                    request.request,
                    ActivationRequest::Activate {
                        slot_id: slot(),
                        expected_generation: u64::MAX,
                    }
                );
                if mode == "oversize" {
                    stream.write_u32(u32::MAX).await.unwrap();
                } else if mode != "lost-reply" {
                    let response = ActivationResponseEnvelope {
                        schema_version: 1,
                        request_id: request.request_id + u64::from(mode == "wrong-id"),
                        response: if mode == "stale" {
                            ActivationResponse::Error {
                                code: ActivationErrorCode::StaleGeneration,
                            }
                        } else {
                            ActivationResponse::AwaitingHealth {
                                receipt: ActivationReceipt {
                                    operation_id: "1".repeat(32),
                                    target_slot_id: if mode == "wrong-target" {
                                        format!("app-00000000000000000002-{}", "b".repeat(64))
                                    } else {
                                        slot()
                                    },
                                },
                            }
                        },
                    };
                    let mut frame =
                        encode_frame(&activation::encode_response(&response).unwrap()).unwrap();
                    if mode == "trailing" {
                        frame.push(0);
                    }
                    stream.write_all(&frame).await.unwrap();
                }
                drop(stream);
                assert!(
                    timeout(Duration::from_millis(30), listener.accept())
                        .await
                        .is_err(),
                    "unexpected retry"
                );
            });
            let result = UpdateActivationClient::new(path)
                .unwrap()
                .activate(&slot(), u64::MAX)
                .await;
            match mode {
                "ok" => assert_eq!(result.unwrap().target_slot_id, slot()),
                "wrong-id" | "wrong-target" => assert!(matches!(
                    result,
                    Err(ActivationClientError::MismatchedResponse)
                )),
                "oversize" | "trailing" => {
                    assert!(matches!(result, Err(ActivationClientError::Protocol(_))))
                }
                "stale" => assert!(matches!(
                    result,
                    Err(ActivationClientError::Rejected(
                        ActivationErrorCode::StaleGeneration
                    ))
                )),
                _ => assert!(matches!(result, Err(ActivationClientError::Io(_)))),
            }
            worker.await.unwrap();
        }
    }

    #[tokio::test]
    async fn validation_precedes_connect_and_timeout_releases_single_admission() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("activate.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let mut client = UpdateActivationClient::new(path).unwrap();
        client.deadline = Duration::from_millis(50);
        for (target, generation) in [("../bad".into(), 1), (slot(), 0)] {
            assert!(matches!(
                client.activate(&target, generation).await,
                Err(ActivationClientError::Protocol(_))
            ));
        }
        assert!(
            timeout(Duration::from_millis(10), listener.accept())
                .await
                .is_err()
        );
        let client = std::sync::Arc::new(client);
        let sending = tokio::spawn({
            let client = client.clone();
            async move { client.activate(&slot(), 1).await }
        });
        let (_stream, _) = listener.accept().await.unwrap();
        assert!(matches!(
            client.activate(&slot(), 1).await,
            Err(ActivationClientError::Busy)
        ));
        assert!(matches!(
            sending.await.unwrap(),
            Err(ActivationClientError::Timeout)
        ));
        assert!(matches!(
            client.activate("bad", 1).await,
            Err(ActivationClientError::Protocol(_))
        ));
    }
}

#[derive(Debug)]
pub struct UpdateActivationClient {
    path: PathBuf,
    next_id: AtomicU64,
    concurrency: Semaphore,
    deadline: Duration,
}

impl UpdateActivationClient {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ActivationClientError> {
        let path = path.into();
        // Reuse the same bounded, absolute Unix socket path policy.
        super::UpdateStatusClient::new(path.clone())
            .map_err(|_| ActivationClientError::Configuration)?;
        Ok(Self {
            path,
            next_id: AtomicU64::new(1),
            concurrency: Semaphore::new(1),
            deadline: Duration::from_secs(10),
        })
    }

    /// Send once. A dropped reply can follow a committed activation: never retry here.
    pub async fn activate(
        &self,
        slot_id: &str,
        expected_generation: u64,
    ) -> Result<ActivationReceipt, ActivationClientError> {
        let _permit = self
            .concurrency
            .try_acquire()
            .map_err(|_| ActivationClientError::Busy)?;
        match timeout(
            self.deadline,
            self.execute(slot_id, expected_generation, false),
        )
        .await
        .map_err(|_| ActivationClientError::Timeout)??
        {
            ActivationResponse::AwaitingHealth { receipt } => Ok(receipt),
            _ => Err(ActivationClientError::MismatchedResponse),
        }
    }

    /// Prepared admission is not candidate selection or health confirmation.
    /// The exchange still requires response EOF and never retries on ambiguity.
    pub async fn admit(
        &self,
        slot_id: &str,
        expected_generation: u64,
    ) -> Result<PreparedActivationReceipt, ActivationClientError> {
        let _permit = self
            .concurrency
            .try_acquire()
            .map_err(|_| ActivationClientError::Busy)?;
        match timeout(
            self.deadline,
            self.execute(slot_id, expected_generation, true),
        )
        .await
        .map_err(|_| ActivationClientError::Timeout)??
        {
            ActivationResponse::Prepared { receipt } => Ok(receipt),
            _ => Err(ActivationClientError::MismatchedResponse),
        }
    }

    async fn execute(
        &self,
        slot_id: &str,
        expected_generation: u64,
        admission: bool,
    ) -> Result<ActivationResponse, ActivationClientError> {
        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.checked_add(1).unwrap_or(1))
            })
            .unwrap_or(1);
        let request = ActivationRequestEnvelope {
            schema_version: UPDATE_SCHEMA_VERSION,
            request_id: id,
            request: if admission {
                ActivationRequest::Admit {
                    slot_id: slot_id.into(),
                    expected_generation,
                }
            } else {
                ActivationRequest::Activate {
                    slot_id: slot_id.into(),
                    expected_generation,
                }
            },
        };
        let frame = encode_frame(&activation::encode_request(&request)?)?;
        let mut stream = UnixStream::connect(&self.path).await?;
        stream.write_all(&frame).await?;
        stream.shutdown().await?;
        let length = stream.read_u32().await? as usize;
        if length == 0 || length > MAX_UPDATE_MESSAGE_BYTES {
            return Err(UpdateProtocolError::InvalidLength.into());
        }
        let mut payload = vec![0; length];
        stream.read_exact(&mut payload).await?;
        if stream.read(&mut [0; 1]).await? != 0 {
            return Err(UpdateProtocolError::InvalidLength.into());
        }
        let response = activation::decode_response(&payload)?;
        if response.request_id != id {
            return Err(ActivationClientError::MismatchedResponse);
        }
        match &response.response {
            ActivationResponse::AwaitingHealth { receipt } => {
                if receipt.target_slot_id != slot_id {
                    return Err(ActivationClientError::MismatchedResponse);
                }
                Ok(response.response)
            }
            ActivationResponse::Prepared { receipt } => {
                if receipt.target_slot_id != slot_id
                    || Some(receipt.prepared_generation) != expected_generation.checked_add(1)
                {
                    return Err(ActivationClientError::MismatchedResponse);
                }
                Ok(response.response)
            }
            ActivationResponse::Error { code } => Err(ActivationClientError::Rejected(*code)),
        }
    }
}
