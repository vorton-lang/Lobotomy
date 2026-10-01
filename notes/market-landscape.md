# Market Landscape: Persistent Agent Organizations

> Status: market scan notes  
> Date: 2026-10-01  
> Purpose: record products and frameworks that overlap with Ember's emerging target shape: a persistent organization runtime above model-specific harnesses, with long-lived roles, session management, shared project state, and a user-facing Manager layer.

## 1. High-level conclusion

The current market already contains strong implementations of several pieces Ember would need:

- always-on gateways and persistent agent sessions;
- agent-to-agent messaging;
- external coding-harness integration;
- visual multi-agent workspaces;
- durable workflow runtimes;
- orchestration ledgers and dynamic delegation;
- persistent user-facing assistants;
- mission-control style UX.

However, no surveyed product clearly combines all of the following into one coherent system:

1. a persistent organization runtime above model-specific harnesses;
2. a Manager whose primary optimization target is latent user intent rather than task completion;
3. explicit separation of Role / Active Session / Durable State;
4. selective presentation and suppression of lower-level agent output;
5. user-state probing, momentum protection, and uncertainty-aware escalation;
6. long-lived Tech Leader / Worker sessions across heterogeneous harnesses;
7. UX designed around organization state, not simply multiple agent chat windows.

The likely opportunity is therefore not to reimplement every lower-level runtime primitive, but to reuse mature infrastructure where possible and concentrate research effort on:

> **Manager + role/session semantics + organizational UX.**

## 2. OpenClaw

OpenClaw is one of the closest matches to the runtime layer.

Its architecture centers on a long-running Gateway acting as a control plane for sessions, routing, channels, multiple agents, background operation, and web / CLI / messaging clients.

It also supports agent-to-agent interaction and persistent sub-agent sessions.

Relevant shape:

~~~text
Harness
  ↓
Harness Adapter
  ↓
Persistent Agent Session
  ↓
Organization Runtime
~~~

OpenClaw also supports ACP-based external coding agents, including model-specific harnesses such as Claude Code, Codex, Cursor, Copilot, Droid, OpenCode, and Gemini CLI. Its acpx tooling is especially relevant as a headless, stateful interface over ACP coding agents.

The main gap relative to Ember is that OpenClaw's control plane primarily manages messaging, routing, and sessions. Ember's Manager is intended to manage latent user intent, information policy, momentum, challenge timing, and organizational behavior.

Questions Ember wants to make first-class include:

- Should this Tech Lead objection be shown to the user now?
- Has the user already closed this design space?
- Is the user losing patience?
- Should the system probe, defer, suppress, or escalate?

References:

- https://docs.openclaw.ai/
- https://docs.openclaw.ai/gateway/config-tools/sessions-and-subagents
- https://docs.openclaw.ai/tools/acp-agents
- https://github.com/openclaw/acpx

## 3. OpenHands Agent Canvas

Agent Canvas is especially relevant to the idea of building above existing harnesses rather than replacing them.

It can drive external coding agents such as Claude Code, Codex, Gemini CLI, and OpenHands agents. The visual workspace manages the outer environment while the underlying harness retains responsibility for model execution, tools, and local execution behavior.

Architecturally, the important pattern is:

~~~text
Agent Canvas
    ↓
Agent Server
    ↓
ACP subprocess
    ↓
Claude Code / Codex / Gemini CLI
~~~

This is close to Ember's desired principle:

> do not rebuild Claude Code or Codex; put an organizational runtime above them.

Agent Canvas also provides precedents for project overview, parallel agent runs, git state, PR/change inspection, remote execution, long-running cloud workspaces, and automation activity.

The main gap is interaction topology. The basic model remains closer to:

~~~text
User → Agent / Task
~~~

rather than:

~~~text
User → Manager → Organization
~~~

The user is still substantially aware of and responsible for operating agents.

References:

- https://www.openhands.dev/product/canvas
- https://github.com/OpenHands/docs/blob/main/openhands/usage/agent-canvas/acp-agents.mdx
- https://www.openhands.dev/blog/what-are-coding-agents
- https://www.openhands.dev/blog/new-in-agent-canvas-august-2026

## 4. Factory

Factory is notable less for foundational architecture and more for its productization of an AI software organization.

Relevant concepts include:

- multiple Droid sessions;
- persistent machines;
- long-lived environments;
- Mission Control;
- Orchestrator / Worker / Validator separation;
- model selection by role;
- pause / redirect / resume;
- mobile review of progress and diffs.

Factory is a strong UX precedent for the idea that a user can supervise multiple active workers without reading raw internal communication.

Its Mission Control overlaps with Ember's emerging Workboard, Agent Inspector, organization status view, and background execution UX.

The main gap is that Factory's Orchestrator is primarily task-oriented:

> How should this mission be completed?

Ember's Manager is intended to answer a higher-level question:

> What does the user currently want, what should they see, and what should the organization do about it?

References:

- https://factory.ai/news/factory-desktop
- https://docs.factory.ai/missions/running-app
- https://factory.ai/product/web

## 5. OpenAI Dots

Dots are notable because their product goal overlaps strongly with the user-facing side of Ember.

The primary dot is described around:

- always-on operation;
- persistent user relationship;
- learning preferences;
- maintaining context across channels;
- managing multiple projects;
- presenting progress, questions, and decisions;
- launching or coordinating work in other OpenAI execution systems.

Structurally this begins to resemble:

~~~text
          User
            │
        Primary Dot
            │
      ┌─────┴─────┐
      ▼           ▼
    Codex      ChatGPT Work
~~~

which is close to:

~~~text
          User
            │
         Manager
         /     \
       TL     Worker
~~~

The main gap relative to Ember is that public descriptions do not clearly expose first-class concepts corresponding to:

- latent-user-state ledger;
- closed/open decisions;
- objection suppression;
- selective lower-level information filtering;
- role/session rotation;
- temporary flattening;
- user-momentum protection.

Reference:

- https://openai.com/index/introducing-dots/

## 6. Microsoft Agent Framework / Magentic

Magentic-style orchestration is useful as a precedent for a Manager-like control loop.

A manager maintains ledgers containing facts, unknowns, hypotheses, plans, progress, stalled state, next speaker, and next instruction. The orchestration system dynamically decides which specialist should act next.

This resembles some of the mechanics Ember's Manager may need:

- explicit state;
- dynamic delegation;
- progress tracking;
- replanning;
- durable workflows;
- checkpoint/resume;
- human-in-the-loop control.

The critical difference is objective.

Magentic's manager is fundamentally a task manager:

> Who should act next to complete the task?

Ember's Manager is intended to be a boss-facing organization manager:

> Does the user still want this task framed this way?  
> Is this information worth interrupting them with?  
> Should the system challenge, probe, suppress, defer, or stop?

References:

- https://microsoft.github.io/autogen/dev/user-guide/agentchat-user-guide/magentic-one.html
- https://learn.microsoft.com/en-us/agent-framework/workflows/orchestrations/magentic
- https://learn.microsoft.com/en-us/agent-framework/integrations/durable-extension

## 7. CrewAI / AutoGen

These systems already provide mature abstractions for:

- role-based agents;
- crews / teams;
- processes;
- shared workflow state;
- persistence;
- resumability;
- group chat;
- visual team builders;
- monitoring and observability.

They show that role-play and task-oriented multi-agent composition are already well explored.

Their dominant abstraction is still:

> Given a task, organize multiple agents to complete it.

Ember is instead converging on:

> Maintain a long-lived organization around one user, across many tasks, changing intentions, partially observed preferences, and heterogeneous harnesses.

References:

- https://docs.crewai.com/core-concepts/Agents
- https://microsoft.github.io/autogen/stable/user-guide/autogenstudio-user-guide/usage.html

## 8. ACP may remove substantial infrastructure work

A major implementation implication of this scan is the growing importance of Agent Client Protocol (ACP).

OpenHands and OpenClaw both use ACP as a common interface over external coding harnesses.

That suggests Ember may not need to independently build:

~~~text
ClaudeAdapter
CodexAdapter
GeminiAdapter
DroidAdapter
...
~~~

for every supported harness.

A preferred route may be:

~~~text
Ember Runtime
     ↓
    ACP
     ↓
Claude / Codex / Gemini / ...
~~~

with custom compatibility adapters only for harnesses that do not expose suitable ACP capabilities.

This should be validated before committing to Ember's own harness abstraction.

## 9. The likely market gap

A rough mapping is:

~~~text
OpenClaw
→ always-on gateway / sessions / agent-to-agent / channels

OpenHands
→ layer above existing harnesses / ACP / workspace

Factory
→ multi-session organizational UX / mission control

Microsoft Magentic
→ manager ledger / dynamic delegation

OpenAI Dots
→ persistent user relationship / top-level assistant
~~~

The missing middle appears to be:

~~~text
                    User
                      │
                      ▼
        ┌─────── Manager ───────┐
        │ latent user model     │
        │ information policy    │
        │ momentum              │
        │ uncertainty           │
        │ selective probing     │
        └──────────┬────────────┘
                   │
          Organization Runtime
             /            \
      persistent         persistent
      TL session         Worker session
~~~

Three especially underexplored areas stand out.

### Manager optimizes user intent, not only task completion

Current orchestrators mainly optimize completion of an already-specified task.

Ember's Manager must also manage whether the task, framing, timing, and amount of information are right for the user.

### Role and session are separate concepts

A Tech Leader role should be able to continue its current session, checkpoint, restart in a clean session, switch model, or switch harness without the role itself disappearing.

### Session rotation mixes cognition and economics

The system may need to jointly consider:

~~~text
stale context
wrong attractor
context pressure
closed-problem residue
~~~

versus:

~~~text
native continuity
local working state
prompt-cache discount
environment continuity
~~~

This tradeoff does not appear to be widely elevated to a first-class orchestration policy.

## 10. Implementation implication

This scan argues against starting Ember by building a completely new agent OS.

A more efficient direction is likely:

- study or reuse OpenClaw-style persistent session infrastructure;
- study OpenHands / ACP for external harness integration;
- study Factory primarily for organizational UX;
- use Magentic-style explicit ledgers where useful;
- build Ember-specific research around Manager behavior, role/session semantics, and UX.

The core research claim can remain narrow and testable:

> **Can a persistent organization with a user-modeling Manager outperform a flat or task-oriented multi-agent system in long-horizon human collaboration?**
