//! What a worker host reports (worker-interface.md §4, §5): an attempt's state, its messages,
//! questions, artifacts and evidence, as events and as the snapshot a subscription starts with.

use serde::{Deserialize, Serialize};

/// The notification that carries an [`Event`].
pub const EVENT: &str = "event";
/// The notification that carries [`Resources`].
pub const RESOURCES: &str = "resources";

/// One change of a task. `seq` rises by one per task; it, not a timestamp, is where a
/// subscription resumes (§9). An event of an attempt that is no longer the task's current one is
/// recorded, never acted on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub task_id: String,
    pub attempt_id: String,
    pub seq: u64,
    #[serde(flatten)]
    pub body: EventBody,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventBody {
    StateChanged {
        state: AttemptState,
        /// Set when `state` is `blocked`.
        blocked: Option<Blocked>,
        reason: Option<String>,
    },
    MessageReceipt {
        message_id: String,
        receipt: Receipt,
    },
    QuestionOpened {
        question: Question,
    },
    QuestionClosed {
        question_id: String,
        /// The message that answered it, if one did.
        answered_by: Option<String>,
    },
    /// The agent's latest action in a line, for the GUI. It wakes nobody.
    Progress {
        line: String,
    },
    /// A new artifact, or a new version of one.
    ArtifactPublished {
        artifact: Artifact,
    },
    EvidenceUpdated {
        evidence: Evidence,
    },
    /// The agent finds the orchestrator's reading at odds with the user's words: what differs,
    /// and which one it follows. The orchestrator shows it to the user as it is, not through
    /// Angela (manager-actions.md §2.3).
    MismatchFlagged {
        body: String,
    },
}

/// An attempt's state (§4). Review rework goes from `delivered` back to `working`; `delivered` is
/// not final.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum AttemptState {
    Submitted,
    Preparing,
    Working,
    /// Waiting for an answer or an authorization: see [`Blocked`].
    Blocked,
    /// An artifact meets `done_when` with its evidence; idle, waiting for review.
    Delivered,
    Adopted,
    Abandoned,
    Failed,
    Canceled,
    /// Lost contact, or the agent's process has not been seen to exit; waiting to be reconciled.
    Unknown,
}

impl AttemptState {
    /// Adopted, abandoned, failed or canceled: the attempt takes no more messages.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Adopted | Self::Abandoned | Self::Failed | Self::Canceled)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Blocked {
    pub need: Need,
    /// The question to answer, for `needs_input`.
    pub question_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum Need {
    #[serde(rename = "needs_input")]
    Input,
    #[serde(rename = "needs_auth")]
    Auth,
    /// The last turn was interrupted or failed, its quota refused included; the state's `reason`
    /// says why. A message goes on in the same session and workspace. Nothing goes on by itself
    /// (§3).
    #[serde(rename = "resume")]
    Resume,
}

/// What became of a message (§5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Receipt {
    Queued,
    /// Bound to a turn, named by an opaque id (§2).
    Bound {
        turn: String,
    },
    Delivered,
    NotDelivered {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Question {
    pub question_id: String,
    pub body: String,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum ArtifactKind {
    /// Code; its version is the head commit.
    Change,
    /// A document; its version is the content hash.
    Document,
    /// Text, inline.
    Report,
}

/// A version of an artifact (§6). A new version leaves the evidence of the old ones as history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Artifact {
    pub artifact_id: String,
    pub version: String,
    pub kind: ArtifactKind,
    /// Another artifact this one replaces. Versions of the same artifact replace each other
    /// without it.
    pub supersedes: Option<String>,
    pub title: String,
    /// Where to see it, such as the pull request.
    pub url: Option<String>,
    pub size: Option<ChangeSize>,
    /// A report's text.
    pub text: Option<String>,
    /// The paths a change touches, for the orchestrator's authorization check (tech-plan.md §4.6).
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct ChangeSize {
    pub files: u32,
    pub additions: u32,
    pub deletions: u32,
}

/// One check or review result on a version of an artifact (§6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Evidence {
    pub artifact_id: String,
    pub version: String,
    pub name: String,
    pub status: EvidenceStatus,
    pub source: EvidenceSource,
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum EvidenceStatus {
    Pending,
    Passed,
    Failed,
}

/// Who says so. Only the environment's evidence meets `done_when`; the agent's own word is a
/// lead, and shown as such (§6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum EvidenceSource {
    Environment,
    Agent,
}

/// A task as it stands: what a subscription starts with (§9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub spec_version: u32,
    pub attempt_id: String,
    pub state: AttemptState,
    pub blocked: Option<Blocked>,
    pub reason: Option<String>,
    /// Set once the task is closed.
    pub closed: Option<crate::ops::CloseOutcome>,
    /// The current attempt's open question.
    pub question: Option<Question>,
    /// The current attempt's messages.
    pub messages: Vec<MessageStatus>,
    /// The current version of each artifact.
    pub artifacts: Vec<Artifact>,
    /// The evidence on those versions.
    pub evidence: Vec<Evidence>,
    /// The task's last event.
    pub seq: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageStatus {
    pub message_id: String,
    pub receipt: Receipt,
}

/// The worker host's resources (§5). Stage 1 reports quota only (tech-plan.md §7).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Resources {
    pub quota: Vec<Quota>,
}

/// A harness's quota as the worker host last saw it. A blocked domain starts no new turn; it
/// opens again only when the user retries (data-model.md §8.3, §8.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    pub harness: String,
    pub blocked: bool,
    /// When it resets, in milliseconds since the epoch, if the harness said.
    pub resets_at: Option<i64>,
    pub message: Option<String>,
}
