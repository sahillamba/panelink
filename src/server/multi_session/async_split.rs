use super::TargetStateSeed;
use crate::control_protocol::TargetRef;
use crate::pane_layout::SplitPreview;
use crate::protocol::OperationKey;

/// Result of starting or observing one owner-serialized async split.
pub(crate) enum AsyncSplitAdmission {
    /// The exact operation already has a terminal result.
    Terminal(crate::split_protocol::SplitResult),
    /// The exact operation remains admitted and in progress.
    Pending,
    /// The worker's outcome is uncertain and no child handle was adopted.
    Uncertain,
    /// The caller owns the one launch-work slot and may enqueue this target.
    Start {
        key: OperationKey,
        target: TargetRef,
    },
}

/// All owner state needed to finish the one in-flight split without reading
/// the request again or trusting worker-supplied topology.
pub(super) struct ActiveAsyncSplit {
    pub(super) key: OperationKey,
    pub(super) existing_target: TargetRef,
    pub(super) new_target: TargetRef,
    pub(super) preview: SplitPreview,
    pub(super) existing_rect: crate::pane_layout::Rect,
    pub(super) dimensions: (u16, u16),
    pub(super) seed: Option<TargetStateSeed>,
    pub(super) child_attached: bool,
}
