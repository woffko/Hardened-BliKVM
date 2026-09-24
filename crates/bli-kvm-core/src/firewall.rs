//! Typed firewall operations. Identity comes from the authenticated server;
//! native ports and the current boot identity remain owned by the broker.
use crate::ControlV2ValidationError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirewallMode {
    Baseline,
    Moderate,
    Restricted,
    Paranoid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FirewallModeStatus {
    pub mode: FirewallMode,
    pub pending_operation_id: Option<[u8; 16]>,
    pub confirmation_deadline_unix_ms: Option<u64>,
    pub recovery_required: bool,
    pub configuration_ready: bool,
}

impl FirewallMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Moderate => "moderate",
            Self::Restricted => "restricted",
            Self::Paranoid => "paranoid",
        }
    }
    pub fn requires_confirmation(self) -> bool {
        matches!(self, Self::Restricted | Self::Paranoid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FirewallChangeRequest {
    pub operation_id: [u8; 16],
    pub session_binding: [u8; 32],
    pub mode: FirewallMode,
    pub issued_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FirewallConfirmRequest {
    pub operation_id: [u8; 16],
    pub session_binding: [u8; 32],
    pub issued_at_unix_ms: u64,
}

fn validate(
    operation: &[u8; 16],
    session: &[u8; 32],
    issued: u64,
) -> Result<(), ControlV2ValidationError> {
    if *operation == [0; 16] || issued == 0 {
        return Err(ControlV2ValidationError::InvalidFirewallOperation);
    }
    if *session == [0; 32] {
        return Err(ControlV2ValidationError::InvalidSessionBinding);
    }
    Ok(())
}

impl FirewallChangeRequest {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        validate(
            &self.operation_id,
            &self.session_binding,
            self.issued_at_unix_ms,
        )
    }
}
impl FirewallConfirmRequest {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        validate(
            &self.operation_id,
            &self.session_binding,
            self.issued_at_unix_ms,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operations_are_strict_and_identity_bound() {
        let mut request = FirewallChangeRequest {
            operation_id: [1; 16],
            session_binding: [2; 32],
            mode: FirewallMode::Restricted,
            issued_at_unix_ms: 100,
        };
        assert!(request.validate().is_ok());
        let mut json = serde_json::to_value(&request).unwrap();
        json["mode"] = serde_json::json!("off");
        assert!(serde_json::from_value::<FirewallChangeRequest>(json).is_err());
        let mut json = serde_json::to_value(&request).unwrap();
        json["httpsPort"] = serde_json::json!(8443);
        assert!(serde_json::from_value::<FirewallChangeRequest>(json).is_err());
        request.session_binding = [0; 32];
        assert!(request.validate().is_err());
        assert!(
            FirewallConfirmRequest {
                operation_id: [0; 16],
                session_binding: [2; 32],
                issued_at_unix_ms: 100
            }
            .validate()
            .is_err()
        );
    }
}
