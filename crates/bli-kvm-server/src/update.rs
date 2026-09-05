use axum::{Json, extract::State};
use bli_kvm_update_protocol::{
    ApplicationRollbackReason, ApplicationSlotVersion, ApplicationUpdateStatus,
    PendingApplicationUpdate, UPDATE_SCHEMA_VERSION, UpdatePendingPhase,
};
use serde::Serialize;

use crate::{
    AppState,
    error::{ApiResponse, AppError, AppResult},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SlotView {
    slot_id: String,
    version: String,
    sequence: String,
    minimum_system_sequence: String,
}

impl From<ApplicationSlotVersion> for SlotView {
    fn from(slot: ApplicationSlotVersion) -> Self {
        Self {
            slot_id: slot.slot_id,
            version: slot.version,
            sequence: slot.sequence.to_string(),
            minimum_system_sequence: slot.minimum_system_sequence.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingView {
    operation_id: String,
    target: SlotView,
    phase: UpdatePendingPhase,
}

impl From<PendingApplicationUpdate> for PendingView {
    fn from(pending: PendingApplicationUpdate) -> Self {
        Self {
            operation_id: pending.operation_id,
            target: pending.target.into(),
            phase: pending.phase,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateStatusView {
    schema_version: u16,
    // Decimal strings preserve exact u64 values through JavaScript clients.
    generation: String,
    floor_sequence: String,
    system_sequence: String,
    selected: SlotView,
    confirmed: SlotView,
    previous: Option<SlotView>,
    pending: Option<PendingView>,
    last_rollback: Option<ApplicationRollbackReason>,
}

impl From<ApplicationUpdateStatus> for UpdateStatusView {
    fn from(status: ApplicationUpdateStatus) -> Self {
        Self {
            schema_version: UPDATE_SCHEMA_VERSION,
            generation: status.generation.to_string(),
            floor_sequence: status.floor_sequence.to_string(),
            system_sequence: status.system_sequence.to_string(),
            selected: status.selected.into(),
            confirmed: status.confirmed.into(),
            previous: status.previous.map(Into::into),
            pending: status.pending.map(Into::into),
            last_rollback: status.last_rollback,
        }
    }
}

pub(crate) async fn status(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<UpdateStatusView>>> {
    let client = state.update_status.as_ref().ok_or_else(|| {
        AppError::ServiceUnavailable("application update status is not configured".into())
    })?;
    let status = client.status().await.map_err(|_| {
        AppError::ServiceUnavailable("application update status is unavailable".into())
    })?;
    Ok(Json(ApiResponse::ok(status.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_u64_counters_without_javascript_rounding() {
        let slot = ApplicationSlotVersion {
            slot_id: format!("app-{:020}-{}", u64::MAX, "a".repeat(64)),
            version: "1.0.0".into(),
            sequence: u64::MAX,
            minimum_system_sequence: 1,
        };
        let status = ApplicationUpdateStatus {
            generation: u64::MAX,
            floor_sequence: u64::MAX,
            system_sequence: 1,
            selected: slot.clone(),
            confirmed: slot,
            previous: None,
            pending: None,
            last_rollback: None,
        };
        let value = serde_json::to_value(UpdateStatusView::from(status)).unwrap();
        assert_eq!(value["generation"], "18446744073709551615");
        assert_eq!(value["floorSequence"], "18446744073709551615");
        assert_eq!(value["selected"]["sequence"], "18446744073709551615");
    }
}
