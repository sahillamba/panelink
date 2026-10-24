//! Owner-side explicit recovery state machine. All network and storage IO uses workers.

use crate::managed_provider_store::ManagedProviderScope;
use crate::provider_bridge_runtime::{ControlReply, ControlSubmitError, ProviderBridgeRuntime};
use crate::provider_recovery_control::{NavigateOutcome, ValidationOutcome};
use crate::provider_recovery_endpoint::{
    ManualReason, RecoveryCommand, RecoveryEndpoint, RecoveryReply, RecoveryRequest,
};
use crate::provider_recovery_prepared::{
    AsyncPreparedResumeWorker, PreparedResumeRecord, PreparedResumeState, PreparedResumeTicket,
    PreparedWorkerCompletion, PreparedWorkerPoll, PreparedWorkerResult, PreparedWorkerSubmit,
    PreparedWorkerToken,
};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VALIDATION_MAX_AGE: Duration = Duration::from_secs(3);

struct Choice {
    id: String,
    ticket: PreparedResumeTicket,
    revision: u64,
    instance: u64,
    spent: bool,
    confirmation_armed: bool,
}

impl Choice {
    fn restore(record: PreparedResumeRecord) -> Self {
        Self {
            id: record.choice_id().to_owned(),
            ticket: record.ticket().clone(),
            revision: record.prospective_revision(),
            instance: record.source_bridge_instance(),
            spent: record.state() == PreparedResumeState::Spent,
            confirmation_armed: false,
        }
    }
}

#[derive(Clone, Copy)]
enum ValidationPurpose {
    Prepare,
    Resume,
}

enum Work {
    Idle,
    Validating {
        request: RecoveryRequest,
        command: u64,
        id: String,
        purpose: ValidationPurpose,
    },
    Saving {
        request: RecoveryRequest,
        token: PreparedWorkerToken,
        id: String,
        revision: u64,
        instance: u64,
    },
    Spending {
        request: RecoveryRequest,
        token: PreparedWorkerToken,
    },
    Dispatching {
        request: RecoveryRequest,
    },
    Navigating {
        request: RecoveryRequest,
        command: u64,
    },
    Confirming {
        request: RecoveryRequest,
    },
}

/// One exact managed target's manual workflow. Drop cancels workers without waiting.
pub struct ManualRecovery {
    endpoint: RecoveryEndpoint,
    writer: AsyncPreparedResumeWorker,
    scope: ManagedProviderScope,
    ready: bool,
    failed: bool,
    choice: Option<Choice>,
    work: Work,
}

impl std::fmt::Debug for ManualRecovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManualRecovery { <redacted> }")
    }
}

impl ManualRecovery {
    /// Startup only: bind the private command endpoint and launch the storage worker.
    pub fn start(
        store_path: &Path,
        endpoint_path: &Path,
        scope: ManagedProviderScope,
    ) -> io::Result<Self> {
        let endpoint = RecoveryEndpoint::bind(endpoint_path, scope.target(), scope.project())?;
        let writer = AsyncPreparedResumeWorker::start(store_path, scope.clone())?;
        Ok(Self {
            endpoint,
            writer,
            scope,
            ready: false,
            failed: false,
            choice: None,
            work: Work::Idle,
        })
    }

    /// Call after `bridge.poll(now)`. At most one request, IO reply, and write completion are consumed.
    pub fn poll(&mut self, bridge: &mut ProviderBridgeRuntime, now: Instant) {
        let mut completion = match self.writer.poll() {
            PreparedWorkerPoll::Ready(record) => {
                self.ready = true;
                self.choice = record.map(Choice::restore);
                None
            }
            PreparedWorkerPoll::InitializationFailed => {
                self.failed = true;
                None
            }
            PreparedWorkerPoll::Completed(done) => Some(done),
            _ => None,
        };
        if self.writer.is_unavailable() {
            self.failed = true;
        }
        let mut control = bridge.poll_control_reply();
        let now = now.max(Instant::now());
        let work = std::mem::replace(&mut self.work, Work::Idle);
        self.work = self.advance(work, bridge, now, &mut control, &mut completion);
        // A completion without an owner operation cannot authorize any action.
        if completion.is_some() || control.is_some() {
            self.failed = true;
        }
        if let Some(request) = self.endpoint.poll() {
            self.accept(request, bridge);
        }
    }

    fn accept(&mut self, request: RecoveryRequest, bridge: &mut ProviderBridgeRuntime) {
        if request.is_cancelled() {
            return;
        }
        if request.target() != self.scope.target() || request.project() != self.scope.project() {
            reply(request, manual(ManualReason::ProjectMismatch));
            return;
        }
        if matches!(request.command(), RecoveryCommand::Status) {
            let status = self.status(bridge);
            reply(request, status);
            return;
        }
        if self.failed {
            reply(request, RecoveryReply::Unavailable);
            return;
        }
        if !self.ready || !matches!(self.work, Work::Idle) {
            reply(request, RecoveryReply::Busy);
            return;
        }
        let (id, purpose) = match request.command() {
            RecoveryCommand::Prepare { session_id } => {
                if self.choice.is_some() {
                    reply(request, manual(ManualReason::InterruptedOperation));
                    return;
                }
                (session_id.to_owned(), ValidationPurpose::Prepare)
            }
            RecoveryCommand::Resume { ticket, revision } => {
                let Some(choice) = self.choice.as_ref() else {
                    reply(request, manual(ManualReason::InvalidChoice));
                    return;
                };
                if choice.spent
                    || choice.ticket.as_bytes() != ticket
                    || choice.revision != *revision
                    || bridge.snapshot().bridge_instance() != Some(choice.instance)
                {
                    reply(request, manual(ManualReason::InterruptedOperation));
                    return;
                }
                (choice.id.clone(), ValidationPurpose::Resume)
            }
            RecoveryCommand::Status => return,
        };
        match bridge.request_validation(&id) {
            Ok(command) => {
                self.work = Work::Validating {
                    request,
                    command,
                    id,
                    purpose,
                }
            }
            Err(ControlSubmitError::Busy) => reply(request, RecoveryReply::Busy),
            Err(_) => reply(request, RecoveryReply::Unavailable),
        }
    }

    fn status(&self, bridge: &ProviderBridgeRuntime) -> RecoveryReply {
        if self.failed {
            return RecoveryReply::Unavailable;
        }
        let Some(choice) = self.choice.as_ref() else {
            return manual(ManualReason::InvalidChoice);
        };
        if choice.spent {
            if !choice.confirmation_armed {
                return RecoveryReply::Uncertain;
            }
            return bridge
                .confirmed_revision_for(&choice.id, choice.revision)
                .map(|revision| RecoveryReply::Confirmed { revision })
                .unwrap_or(RecoveryReply::Uncertain);
        }
        if bridge.snapshot().bridge_instance() != Some(choice.instance) {
            return manual(ManualReason::InterruptedOperation);
        }
        RecoveryReply::Prepared {
            ticket: *choice.ticket.as_bytes(),
            revision: choice.revision,
        }
    }

    fn advance(
        &mut self,
        work: Work,
        bridge: &mut ProviderBridgeRuntime,
        now: Instant,
        control: &mut Option<ControlReply>,
        completion: &mut Option<PreparedWorkerCompletion>,
    ) -> Work {
        match work {
            Work::Idle => Work::Idle,
            Work::Validating {
                request,
                command,
                id,
                purpose,
            } => {
                let Some(result) = control.take() else {
                    return Work::Validating {
                        request,
                        command,
                        id,
                        purpose,
                    };
                };
                if request.is_cancelled() {
                    return Work::Idle;
                }
                let (outcome, received_at) = match result {
                    ControlReply::Validated {
                        request_id,
                        outcome,
                        received_at,
                    } if request_id == command => (outcome, received_at),
                    ControlReply::Failed { request_id } if request_id == command => {
                        reply(request, RecoveryReply::Unavailable);
                        return Work::Idle;
                    }
                    _ => {
                        self.failed = true;
                        reply(request, RecoveryReply::Uncertain);
                        return Work::Idle;
                    }
                };
                if now
                    .checked_duration_since(received_at)
                    .is_none_or(|age| age >= VALIDATION_MAX_AGE)
                {
                    reply(request, manual(ManualReason::UncertainProvider));
                    return Work::Idle;
                }
                match outcome {
                    ValidationOutcome::Verified(proof)
                        if proof.session_id() == id
                            && proof.project_identity() == self.scope.project() => {}
                    ValidationOutcome::Missing => {
                        reply(request, manual(ManualReason::MissingSession));
                        return Work::Idle;
                    }
                    ValidationOutcome::ForeignProject => {
                        reply(request, manual(ManualReason::ForeignProject));
                        return Work::Idle;
                    }
                    ValidationOutcome::Child => {
                        reply(request, manual(ManualReason::ChildSession));
                        return Work::Idle;
                    }
                    ValidationOutcome::Unsupported => {
                        reply(request, manual(ManualReason::UnsupportedProvider));
                        return Work::Idle;
                    }
                    _ => {
                        reply(request, manual(ManualReason::UncertainProvider));
                        return Work::Idle;
                    }
                }
                match purpose {
                    ValidationPurpose::Prepare => {
                        let (Some(revision), Some(instance)) = (
                            bridge.next_binding_revision(),
                            bridge.snapshot().bridge_instance(),
                        ) else {
                            reply(request, RecoveryReply::Unavailable);
                            return Work::Idle;
                        };
                        match self
                            .writer
                            .submit_prepare(&id, revision, instance, unix_ms())
                        {
                            PreparedWorkerSubmit::Accepted(token) => Work::Saving {
                                request,
                                token,
                                id,
                                revision,
                                instance,
                            },
                            _ => {
                                reply(request, RecoveryReply::Unavailable);
                                Work::Idle
                            }
                        }
                    }
                    ValidationPurpose::Resume => {
                        let Some(choice) = self.choice.as_ref() else {
                            reply(request, RecoveryReply::Uncertain);
                            return Work::Idle;
                        };
                        match self.writer.submit_spend(&choice.ticket, choice.revision) {
                            PreparedWorkerSubmit::Accepted(token) => {
                                Work::Spending { request, token }
                            }
                            _ => {
                                reply(request, RecoveryReply::Unavailable);
                                Work::Idle
                            }
                        }
                    }
                }
            }
            Work::Saving {
                request,
                token,
                id,
                revision,
                instance,
            } => {
                let Some(mut done) = completion.take() else {
                    return Work::Saving {
                        request,
                        token,
                        id,
                        revision,
                        instance,
                    };
                };
                if done.token() != token {
                    self.failed = true;
                    reply(request, RecoveryReply::Uncertain);
                    return Work::Idle;
                }
                match done.take_result() {
                    Some(Ok(PreparedWorkerResult::Prepared(receipt)))
                        if receipt.prospective_revision() == revision =>
                    {
                        let ticket = receipt.ticket().clone();
                        self.choice = Some(Choice {
                            id,
                            ticket: ticket.clone(),
                            revision,
                            instance,
                            spent: false,
                            confirmation_armed: false,
                        });
                        reply(
                            request,
                            RecoveryReply::Prepared {
                                ticket: *ticket.as_bytes(),
                                revision,
                            },
                        );
                    }
                    _ => {
                        self.failed = true;
                        reply(request, RecoveryReply::Unavailable);
                    }
                }
                Work::Idle
            }
            Work::Spending { request, token } => {
                let Some(mut done) = completion.take() else {
                    return Work::Spending { request, token };
                };
                if done.token() != token {
                    self.failed = true;
                    reply(request, RecoveryReply::Uncertain);
                    return Work::Idle;
                }
                let Some(choice) = self.choice.as_mut() else {
                    self.failed = true;
                    reply(request, RecoveryReply::Uncertain);
                    return Work::Idle;
                };
                match done.take_result() {
                    Some(Ok(PreparedWorkerResult::Spent(receipt)))
                        if receipt.prospective_revision() == choice.revision =>
                    {
                        choice.spent = true;
                        if request.is_cancelled() {
                            Work::Idle
                        } else {
                            Work::Dispatching { request }
                        }
                    }
                    _ => {
                        self.failed = true;
                        reply(request, RecoveryReply::Uncertain);
                        Work::Idle
                    }
                }
            }
            Work::Dispatching { request } => {
                if request.is_cancelled() {
                    return Work::Idle;
                }
                let Some(choice) = self.choice.as_ref() else {
                    self.failed = true;
                    reply(request, RecoveryReply::Uncertain);
                    return Work::Idle;
                };
                if !choice.spent {
                    self.failed = true;
                    reply(request, RecoveryReply::Uncertain);
                    return Work::Idle;
                }
                match bridge.request_navigation(
                    &choice.id,
                    choice.revision,
                    *choice.ticket.as_bytes(),
                    request.cancellation_flag(),
                ) {
                    Ok(command) => Work::Navigating { request, command },
                    Err(ControlSubmitError::Busy) => Work::Dispatching { request },
                    Err(_) => {
                        reply(request, RecoveryReply::Uncertain);
                        Work::Idle
                    }
                }
            }
            Work::Navigating { request, command } => {
                let Some(result) = control.take() else {
                    return Work::Navigating { request, command };
                };
                match result {
                    ControlReply::Navigated {
                        request_id,
                        outcome: NavigateOutcome::ActionIssued,
                        ..
                    } if request_id == command => {
                        if let Some(choice) = self.choice.as_mut() {
                            choice.confirmation_armed = true;
                        }
                        bridge.request_reconciliation();
                        if request.is_cancelled() {
                            Work::Idle
                        } else {
                            Work::Confirming { request }
                        }
                    }
                    _ => {
                        reply(request, RecoveryReply::Uncertain);
                        Work::Idle
                    }
                }
            }
            Work::Confirming { request } => {
                if request.is_cancelled() {
                    return Work::Idle;
                }
                let Some(choice) = self.choice.as_ref() else {
                    reply(request, RecoveryReply::Uncertain);
                    return Work::Idle;
                };
                if let Some(revision) = bridge.confirmed_revision_for(&choice.id, choice.revision) {
                    reply(request, RecoveryReply::Confirmed { revision });
                    Work::Idle
                } else if bridge.next_binding_revision().is_none() {
                    reply(request, RecoveryReply::Uncertain);
                    Work::Idle
                } else {
                    Work::Confirming { request }
                }
            }
        }
    }
}

fn manual(reason: ManualReason) -> RecoveryReply {
    RecoveryReply::ManualRequired { reason }
}
fn reply(request: RecoveryRequest, result: RecoveryReply) {
    let _ = request.respond(result);
}
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|t| u64::try_from(t.as_millis()).ok())
        .unwrap_or(0)
}
