//! Explicit adoption of the committed display branch at a savepoint (#67).
//! Observation, the user's intent and the final revision CAS are separate transactions.
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::item::{Blob, record_blobs};
use crate::project::{current_config, require_project};
use crate::verify::CheckOutcome;
use crate::{Caller, Command, Cx, Error, Result};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum DisplayState {
    Current,
    Changed { commit: String },
    Blocked { commit: Option<String>, reason: String },
}

impl DisplayState {
    fn allows_start(&self, previewed: &str) -> bool {
        match self {
            Self::Current => true,
            Self::Blocked { commit: Some(commit), .. } => commit == previewed,
            _ => false,
        }
    }
}

pub fn observation(conn: &Connection) -> Result<DisplayState> {
    let state: Option<String> =
        conn.query_row("SELECT state FROM display_observation WHERE singleton = 1", [], |r| r.get(0)).optional()?;
    match state {
        Some(state) => Ok(serde_json::from_str(&state)?),
        None => Ok(DisplayState::Current),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ObserveDisplay {
    pub state: DisplayState,
}
impl Command for ObserveDisplay {
    const NAME: &'static str = "observe_display";
    type Output = ();
    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let changed = observation(cx.tx)? != self.state;
        cx.tx.execute(
            "INSERT INTO display_observation VALUES (1, ?1)
             ON CONFLICT(singleton) DO UPDATE SET state = excluded.state",
            params![serde_json::to_string(&self.state)?],
        )?;
        if changed {
            cx.emit("display.observed", "project", json!({ "state": self.state }))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Adoption {
    pub id: String,
    pub target: String,
    pub previous: String,
    pub expected_rev: i64,
    pub config_version: i64,
    pub state: String,
    pub error: Option<String>,
}
fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Adoption> {
    Ok(Adoption {
        id: r.get(0)?,
        target: r.get(1)?,
        previous: r.get(2)?,
        expected_rev: r.get(3)?,
        config_version: r.get(4)?,
        state: r.get(5)?,
        error: r.get(6)?,
    })
}
const SELECT: &str = "SELECT id, target, previous, expected_rev, config_version, state, error FROM display_adoption";
pub fn pending(conn: &Connection) -> Result<Option<Adoption>> {
    Ok(conn.query_row(&format!("{SELECT} WHERE state = 'pending'"), [], from_row).optional()?)
}
pub fn load(conn: &Connection, id: &str) -> Result<Adoption> {
    Ok(conn.query_row(&format!("{SELECT} WHERE id = ?1"), [id], from_row)?)
}

pub fn require_no_adoption(conn: &Connection) -> Result<()> {
    if pending(conn)?.is_some() {
        return Err(Error::rejected("adoption_pending", "正在采用展示分支，暂不启动新工作"));
    }
    Ok(())
}
pub fn require_current(conn: &Connection) -> Result<()> {
    require_no_adoption(conn)?;
    if !observation(conn)?.allows_start(&require_project(conn)?.previewed) {
        return Err(Error::rejected("display_changed", "已排队，处理展示分支变化后开始"));
    }
    Ok(())
}

/// SQL half of the savepoint. The runtime also checks live handles and holds its scheduling gate.
pub fn require_savepoint(conn: &Connection) -> Result<()> {
    let busy: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM task WHERE phase IN ('executing', 'verifying', 'accepting'))
         OR EXISTS(SELECT 1 FROM turn WHERE state IN ('registered', 'running', 'unknown'))
         OR EXISTS(SELECT 1 FROM workspace WHERE state = 'materializing')
         OR EXISTS(SELECT 1 FROM verification WHERE state = 'running')
         OR EXISTS(SELECT 1 FROM capture WHERE state = 'intent')",
        [],
        |r| r.get(0),
    )?;
    if busy {
        return Err(Error::rejected("not_at_savepoint", "等待执行者退出、采集保存及任务验收完成；暂停不等于保存点"));
    }
    for role in crate::role::list_roles(conn)? {
        match crate::workspace::require_slot_free(conn, &role.name, None) {
            Ok(()) => {}
            Err(Error::Rejected { code: "outside_changes", .. }) => {
                return Err(Error::rejected(
                    "not_at_savepoint",
                    format!("{} 有未决定的任务外改动，请先处理", role.name),
                ));
            }
            Err(Error::Rejected { .. }) => {
                return Err(Error::rejected(
                    "not_at_savepoint",
                    format!("等待 {} 的 turn 结束并保存采集结果", role.name),
                ));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct AdoptDisplay {
    pub request_id: String,
    pub target: String,
    pub expected_rev: i64,
    /// Unpreviewed publication commits the user confirmed will leave the integration baseline.
    pub acknowledged: Vec<String>,
}
impl Command for AdoptDisplay {
    const NAME: &'static str = "adopt_display";
    type Output = String;
    fn idem_key(&self) -> Option<&str> {
        Some(&self.request_id)
    }
    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<String> {
        caller.require_user()?;
        require_no_adoption(cx.tx)?;
        require_savepoint(cx.tx)?;
        let p = require_project(cx.tx)?;
        if p.integration_rev != self.expected_rev
            || observation(cx.tx)? != (DisplayState::Changed { commit: self.target.clone() })
        {
            return Err(Error::rejected("stale_display", "展示提交或内部版本已变化，请查看最新状态后采用"));
        }
        let mut expected: Vec<_> = unpreviewed(cx.tx)?.into_iter().map(|p| p.commit).collect();
        let mut acknowledged = self.acknowledged.clone();
        expected.sort_unstable();
        acknowledged.sort_unstable();
        if expected != acknowledged {
            return Err(Error::rejected(
                "unpreviewed_publications",
                "请确认尚未展示的已验收成果将退出集成版本；清单变化后需重新确认",
            ));
        }
        let id = crate::id::new_id("adopt");
        cx.tx.execute(
            "INSERT INTO display_adoption (id,target,previous,expected_rev,config_version,state,created_at)
             VALUES (?1,?2,?3,?4,?5,'pending',?6)",
            params![id, self.target, p.integration, p.integration_rev, current_config(cx.tx)?.0, cx.now],
        )?;
        cx.emit("display.adoption_started", &id, json!({ "target": self.target, "previous": p.integration }))?;
        Ok(id)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FinishAdoption {
    pub id: String,
    pub checks: Vec<CheckOutcome>,
    pub blobs: Vec<Blob>,
    pub error: Option<String>,
}
impl Command for FinishAdoption {
    const NAME: &'static str = "finish_adoption";
    type Output = ();
    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<()> {
        caller.require_runtime()?;
        let a = load(cx.tx, &self.id)?;
        if a.state != "pending" {
            return Ok(());
        }
        record_blobs(cx.tx, &self.blobs, cx.now)?;
        let p = require_project(cx.tx)?;
        let (config_version, config) = current_config(cx.tx)?;
        let observed = observation(cx.tx)?;
        let savepoint_error = match require_savepoint(cx.tx) {
            Ok(()) => None,
            Err(Error::Rejected { message, .. }) => Some(message),
            Err(e) => return Err(e),
        };
        let error = self.error.clone().or_else(|| {
            if p.integration_rev != a.expected_rev || p.integration != a.previous || config_version != a.config_version
            {
                Some("内部版本或检查配置已变化，请重新采用".into())
            } else if observed != (DisplayState::Changed { commit: a.target.clone() }) {
                Some("展示目录已变化，请查看最新状态后采用".into())
            } else if savepoint_error.is_some() {
                savepoint_error
            } else if self.checks.len() != config.checks.len()
                || self.checks.iter().zip(&config.checks).any(|(a, b)| a.command != b.command || !a.passed())
            {
                Some(match self.checks.iter().find(|c| !c.passed()) {
                    Some(c) => format!("检查失败：{}\n{}", c.command, c.tail),
                    None => "展示提交未完成项目检查；内部基线保持不变".into(),
                })
            } else {
                None
            }
        });
        let state = if error.is_some() { "failed" } else { "applied" };
        cx.tx.execute(
            "UPDATE display_adoption SET state=?2,checks=?3,error=?4,finished_at=?5 WHERE id=?1",
            params![a.id, state, serde_json::to_string(&self.checks)?, error, cx.now],
        )?;
        if error.is_none() {
            cx.tx.execute("UPDATE project SET integration=?1,integration_rev=integration_rev+1,previewed=?1 WHERE integration_rev=?2",
                params![a.target,a.expected_rev])?;
            cx.tx.execute("UPDATE outbox SET state='superseded',done_at=?1 WHERE kind='preview' AND state IN ('pending','stopped')", [cx.now])?;
            cx.tx.execute(
                "UPDATE display_observation SET state=?1 WHERE singleton=1",
                params![serde_json::to_string(&DisplayState::Current)?],
            )?;
        }
        cx.emit("display.adoption_finished", &a.id, json!({ "target": a.target, "state": state, "error": error }))?;
        Ok(())
    }
}

/// Accepted publications whose preview has not reached the display directory. Superseded
/// previews are already covered by a later preview or an explicit adoption, so are historical.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct UnpreviewedPublication {
    pub task_id: String,
    pub title: String,
    pub commit: String,
}
pub fn unpreviewed(conn: &Connection) -> Result<Vec<UnpreviewedPublication>> {
    let mut stmt = conn.prepare(
        "SELECT p.task_id, t.title, p.commit_id FROM publication p JOIN task t ON t.id=p.task_id
         WHERE EXISTS(SELECT 1 FROM outbox o WHERE o.kind='preview' AND o.state IN ('pending','stopped')
           AND json_extract(o.payload, '$.target')=p.commit_id) ORDER BY p.rev",
    )?;
    Ok(stmt
        .query_map([], |r| Ok(UnpreviewedPublication { task_id: r.get(0)?, title: r.get(1)?, commit: r.get(2)? }))?
        .collect::<std::result::Result<_, _>>()?)
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct DisplayView {
    pub state: DisplayState,
    pub expected_rev: i64,
    pub busy: bool,
    pub start_blocked: bool,
    pub unpreviewed: Vec<UnpreviewedPublication>,
    pub unavailable: Option<String>,
    pub error: Option<String>,
}
pub fn view(conn: &Connection) -> Result<DisplayView> {
    let state = observation(conn)?;
    let p = require_project(conn)?;
    let busy = pending(conn)?.is_some();
    let error = match &state {
        DisplayState::Changed { commit } | DisplayState::Blocked { commit: Some(commit), .. } => conn
            .query_row(
                "SELECT error FROM display_adoption WHERE target=?1 ORDER BY rowid DESC LIMIT 1",
                [commit],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten(),
        _ => None,
    };
    let unavailable = match require_savepoint(conn) {
        Ok(()) => None,
        Err(Error::Rejected { message, .. }) => Some(message),
        Err(e) => return Err(e),
    };
    Ok(DisplayView {
        start_blocked: busy || !state.allows_start(&p.previewed),
        state,
        expected_rev: p.integration_rev,
        busy,
        unpreviewed: unpreviewed(conn)?,
        unavailable,
        error,
    })
}
