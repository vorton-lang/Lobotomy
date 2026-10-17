//! What the orchestrator asks of a worker host (worker-interface.md §3), and the capabilities a
//! worker host declares (§8).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Method;
use crate::event::{ArtifactKind, AttemptState, EvidenceSource, TaskSnapshot};

/// The first call: agree on a protocol version, and learn what the worker host can do. What it
/// does not declare counts as unsupported (§8).
pub struct Hello;

impl Method for Hello {
    const NAME: &'static str = "hello";
    type Params = HelloParams;
    type Result = HelloResult;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HelloParams {
    /// The versions the orchestrator speaks.
    pub protocols: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HelloResult {
    /// The version the worker host chose from `protocols`.
    pub protocol: String,
    pub capabilities: Capabilities,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub agent: AgentCapabilities,
    pub environment: EnvironmentCapabilities,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentCapabilities {
    /// The harness the agent runs on, such as `claude` or `codex`.
    pub harness: String,
    /// The models it may be asked for; empty when it runs the harness's default.
    pub models: Vec<String>,
    /// Whether an attempt goes on in the same session after its turn ends, as review rework
    /// does (§0 rule 4).
    pub resume: bool,
    /// Whether a message can go into a running turn; otherwise it waits for the next turn.
    pub steer: bool,
    /// Whether a running turn can be interrupted: `stop` at the `turn` level.
    pub interrupt: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentCapabilities {
    pub artifacts: Vec<ArtifactKind>,
    pub evidence: Vec<EvidenceSource>,
    /// How far attempts are kept apart from each other and from the machine, such as
    /// `local-folder` (tech-plan.md §2.3).
    pub isolation: String,
    /// The OS the environment runs on, such as `windows` or `linux`.
    pub platform: String,
    /// Whether the environment carries out `adopt`.
    pub adopt: bool,
}

/// A task as the orchestrator hands it over (tech-plan.md §2.2): what to solve, what it may
/// touch, what counts as done and when to come back. A new version comes with `revise`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub title: String,
    /// The user's words, quoted.
    pub user_words: String,
    /// The orchestrator's reading of them.
    pub interpretation: String,
    pub actions: Vec<Action>,
    /// What counts as done, in words.
    pub done_when: String,
    /// The checks that must pass on the delivered version, such as `ci-ok`. Only evidence from
    /// the environment satisfies them (§6).
    pub required_checks: Vec<String>,
    /// When to stop and report instead of going further, in words.
    pub stop_when: String,
    pub budget: Budget,
    /// When to stop and ask.
    pub ask_when: String,
}

/// What the agent may do on the task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ReadOnly,
    EditDocs,
    EditCode,
    /// Deliver the change as a pull request.
    OpenPullRequest,
}

/// What the worker host enforces: an attempt that uses it up ends as `failed`, saying so.
/// Waiting in `blocked` uses none of it (tech-plan.md §2.2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    /// Turns of the attempt, CI rework included. `None`: no limit.
    pub turns: Option<u32>,
    /// Times a CI failure goes back to the agent (§6). `None`: no limit.
    pub ci_rounds: Option<u32>,
}

/// Starts a task with attempt 1. An invalid spec, or one asking for what the worker host did not
/// declare, fails here and starts nothing (§3).
pub struct CreateTask;

impl Method for CreateTask {
    const NAME: &'static str = "create_task";
    type Params = CreateTaskParams;
    type Result = AttemptRef;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTaskParams {
    pub task_id: String,
    pub spec_version: u32,
    pub spec: TaskSpec,
}

/// The attempt a call acted on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttemptRef {
    pub attempt_id: String,
}

/// A message to the task's current attempt. The reply only says it is stored and queued; its
/// binding and delivery come as `message_receipt` events (§3).
pub struct SendMessage;

impl Method for SendMessage {
    const NAME: &'static str = "send_message";
    type Params = SendMessageParams;
    type Result = AttemptRef;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendMessageParams {
    pub task_id: String,
    pub message_id: String,
    pub body: String,
    /// What the message refers to, such as an artifact version under review.
    #[serde(default)]
    pub refs: Vec<String>,
    /// The question it answers.
    pub question_id: Option<String>,
    /// The stop it replies to (`Blocked::stop_id`): the attempt goes on from there. A stop the
    /// attempt is no longer at, or one already replied to, refuses the message.
    #[serde(default)]
    pub stop_id: Option<String>,
}

/// The requirement changed: in one step, stop the current attempt and start a new one on the new
/// spec (§3).
pub struct Revise;

impl Method for Revise {
    const NAME: &'static str = "revise";
    type Params = CreateTaskParams;
    type Result = Revised;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Revised {
    pub attempt_id: String,
    /// Messages not yet delivered to the old attempt, returned as superseded. They do not go to
    /// the new one; the orchestrator decides what of them goes into the new spec.
    pub superseded: Vec<String>,
}

/// Stops the current turn or the whole attempt. The reply names the attempt; the stop has taken
/// effect once the agent's process has exited, which the state events confirm (§3).
pub struct Stop;

impl Method for Stop {
    const NAME: &'static str = "stop";
    type Params = StopParams;
    type Result = Stopped;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StopParams {
    pub task_id: String,
    pub level: StopLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopLevel {
    /// Interrupt the running turn; the attempt stays where it can go on.
    Turn,
    /// Cancel the attempt.
    Attempt,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stopped {
    pub attempt_id: String,
    /// The attempt's state when the reply was sent. Stopping an attempt in a final state changes
    /// nothing (§9).
    pub state: AttemptState,
}

/// The orchestrator adopts a version of an artifact; the environment carries it out, as a merge
/// for code. Compare-and-swap: the version must still be current and the required evidence must
/// have passed on it (§6). Idempotent by artifact and version (§9).
pub struct Adopt;

impl Method for Adopt {
    const NAME: &'static str = "adopt";
    type Params = AdoptParams;
    type Result = AdoptOutcome;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdoptParams {
    pub task_id: String,
    pub artifact_id: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AdoptOutcome {
    Adopted,
    Rejected { reason: AdoptRejection, detail: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptRejection {
    /// The version is no longer the artifact's current one.
    VersionChanged,
    /// Required evidence has not passed on this version.
    EvidenceMissing,
    Conflict,
}

/// Ends the task: frees what it holds, and keeps or cleans its workspace by policy (§3).
pub struct Close;

impl Method for Close {
    const NAME: &'static str = "close";
    type Params = CloseParams;
    type Result = Ack;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloseParams {
    pub task_id: String,
    pub outcome: CloseOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseOutcome {
    Adopted,
    Abandoned,
}

/// A reply that says only that the call took effect.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ack {}

/// Starts the event stream: the reply is the authoritative snapshot of every task; then come the
/// events after it as `event` notifications (§9). The state is the snapshot's, never rebuilt from
/// events.
pub struct Subscribe;

impl Method for Subscribe {
    const NAME: &'static str = "subscribe";
    type Params = SubscribeParams;
    type Result = Snapshot;
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SubscribeParams {
    /// By task, the last event seq the orchestrator has. The worker host first resends the
    /// task's later events it still has, then goes on live. A task not named gets only what
    /// comes after the snapshot.
    #[serde(default)]
    pub after: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub tasks: Vec<TaskSnapshot>,
}
