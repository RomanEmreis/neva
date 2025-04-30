//! Progress notification

use serde::{Serialize, Deserialize};
use crate::types::notification::Notification;
use crate::types::ProgressToken;

/// An out-of-band notification used to inform the receiver of a progress update for a long-running request.
/// 
/// See the [schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/) for details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressNotification {
    /// The progress token which was given in the initial request, 
    /// used to associate this notification with the request that is proceeding.
    #[serde(rename = "progressToken")]
    pub progress_token: ProgressToken,
    
    /// The progress thus far. This should increase every time progress is made, 
    /// even if the total is unknown.
    pub progress: f64,
    
    /// Total number of items to a process (or total progress required), if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
}

impl From<ProgressNotification> for Notification {
    #[inline]
    fn from(progress: ProgressNotification) -> Self {
        Self::new(
            super::commands::PROGRESS, 
            serde_json::to_value(progress).ok()
        )
    }
}