//! What the runtime tells an executor, in the words it reads (#16). The commands decide when to say
//! something; this module says it, so the wording can be read and changed in one place, and can
//! later differ by harness. The GUI reads two shapes back: a message's header (`header`) and the
//! opening of the runtime's note (frontend `continueNote`).

use crate::task::Assignment;
use crate::turn::FailureKind;

/// The current lifecycle snapshot accompanies old messages even in a fresh native session.
pub(crate) fn open_questions(questions: &[crate::question::Question], messages: &[crate::task::Message]) -> String {
    let lines = questions.iter().map(|q| {
        let state = if messages.iter().any(|m| q.accepts(m)) {
            "本 turn 交付了提问之后的回复；若仍需回答，请再次提问"
        } else {
            "仍在等待回答；本 turn 的消息不能当作对这个问题的回复或授权"
        };
        format!("- {}：{}\n  {state}", q.id, q.body)
    });
    format!(
        "【Lobotomy：当前问题】\n{}\n关联只表示后续消息已经交付，不代表问题已解决，也不扩大动作权限。",
        lines.collect::<Vec<_>>().join("\n")
    )
}

pub(crate) fn question_reply(message: &crate::task::Message, questions: &[crate::question::Question]) -> String {
    let replies: Vec<&str> = questions.iter().filter(|q| q.accepts(message)).map(|q| q.id.as_str()).collect();
    if replies.is_empty() {
        format!("\n（消息 {}：仅作上下文，未关联当前问题）", message.id)
    } else {
        format!("\n（消息 {}：提问之后的回复，关联 {}）", message.id, replies.join("、"))
    }
}

/// The manager's name, as messages and briefs name her (roles-and-tasks.md §1.1).
pub const MANAGER: &str = "Angela";

const HOW_TO_REPORT: &str = "完成后调用 org_report，status 为 done。\
     遇到需要用户决定的问题时，调用 org_report，status 为 blocked，\
     并在 blocked_on 中写明问题。";

/// The task as the executor first sees it: the user's words and the current criteria. The first
/// line doubles as its summary in the GUI.
pub(crate) fn brief(title: &str, body: &str, seq: i64, version: i64, criteria: &str) -> String {
    format!(
        "任务：{title}（第 {seq} 轮执行）\n\n\
         用户原话：\n{body}\n\n\
         完成条件（第 {version} 版）：\n{criteria}\n\n\
         {HOW_TO_REPORT}"
    )
}

/// The brief of a task the manager assigned: the user's words she quoted, her interpretation, what
/// she allows and when to stop (roles-and-tasks.md §4), then the criteria.
pub(crate) fn assigned_brief(
    title: &str,
    quoted: &str,
    assignment: &Assignment,
    seq: i64,
    version: i64,
    criteria: &str,
) -> String {
    let actions: Vec<&str> = assignment.actions.iter().map(|a| a.label()).collect();
    let words = match quoted {
        "" => format!("【{MANAGER}】\n{}", assignment.interpretation),
        quoted => forwarded(quoted, &assignment.interpretation),
    };
    format!(
        "任务：{title}（第 {seq} 轮执行，由 {MANAGER} 派出）\n\n\
         {words}\n\n\
         允许的动作：{}\n\
         停止条件：{}\n\n\
         完成条件（第 {version} 版）：\n{criteria}\n\n\
         只做允许的动作。到了停止条件就停下来汇报，不再扩大范围。{QUOTES_FIRST}\n\n\
         {HOW_TO_REPORT}",
        actions.join("、"),
        assignment.stop_when,
    )
}

/// What the executor does when the user's words and the manager's reading of them differ
/// (manager-actions.md §1).
const QUOTES_FIRST: &str =
    "【用户原话】由运行时原样插入，其余是 Angela 的解读。两者冲突时，以原话为准，并调用 flag_mismatch 说明哪里不一致。";

/// One quoted message of the user, under the runtime's header (manager-actions.md §1).
pub(crate) fn quoted(at: &str, body: &str) -> String {
    format!("【用户原话】{at}\n{body}")
}

/// The manager's ledger entries she cites, as recorded, each with the user's words it rests on
/// (manager-actions.md §1 `ledger`, §3.4).
pub(crate) fn cited(entries: &[crate::ledger::CitedEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = entries
        .iter()
        .map(|e| {
            let words: Vec<String> = e.evidence.iter().map(|q| format!("「{}」", q.body)).collect();
            let because =
                if words.is_empty() { String::new() } else { format!("；用户原话：{}", words.join("、")) };
            format!("- [{}] {}（{}{because}）", e.kind.label(), e.statement, e.id)
        })
        .collect();
    format!("【记录】\n{}", lines.join("\n"))
}

/// The manager's words after the user's she quotes. Without quotes her words are all there is, and
/// the message's header already names her.
pub(crate) fn forwarded(quoted: &str, interpretation: &str) -> String {
    if quoted.is_empty() {
        return interpretation.to_owned();
    }
    format!("{quoted}\n\n【{MANAGER} 解读】\n{interpretation}")
}

/// Opens the brief a new native session gets in the middle of an attempt.
pub(crate) const NEW_SESSION: &str = "（新会话）";

pub(crate) fn criteria_updated(version: i64, text: &str) -> String {
    format!("完成条件已更新为第 {version} 版：\n{text}")
}

/// Who a message is from, as its header names them: `source` is a caller scope.
pub(crate) fn sender(source: &str) -> &str {
    match source {
        "user" => "用户",
        "runtime" => "Lobotomy",
        "manager" => MANAGER,
        other => other.strip_prefix("role:").unwrap_or(other),
    }
}

/// A message's header in a turn's input.
pub(crate) fn header(from: &str, task: Option<&str>) -> String {
    match task {
        Some(task) => format!("【来自 {from} · 任务 {task}】"),
        None => format!("【来自 {from}】"),
    }
}

/// What the user chose a message replies to (manager-actions.md §1.1), under its header.
pub(crate) fn replying_to(target: &crate::task::Replied) -> String {
    let first: String = target.text.lines().next().unwrap_or_default().chars().take(60).collect();
    format!("（回复 {} 的 {}：「{first}」）", target.from, target.id)
}

/// What the user was replying to, as a forwarded message carries it (manager-actions.md §1.1).
pub(crate) fn context(header: &str, body: &str) -> String {
    match body {
        "" => format!("【用户当时在回复】{header}"),
        body => format!("【用户当时在回复】{header}\n{body}"),
    }
}

/// The header of the user's message in the manager's input: it names the message, so that she can
/// quote it by its id (manager-actions.md §1, §2.1).
pub(crate) fn header_with_id(from: &str, id: &str) -> String {
    format!("【来自 {from} · {id}】")
}

/// Opens each input of the manager: the user's local time, a weak signal of the time of day
/// (roadmap.md M4). `at` is `YYYY-MM-DD HH:MM`; `weekday` counts from Sunday, 0.
pub(crate) fn local_time(at: &str, weekday: i64) -> String {
    const WEEKDAYS: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
    format!("【用户的本地时间】{at}（周{}）", WEEKDAYS[weekday.rem_euclid(7) as usize])
}

/// The runtime's note at the start of a turn that continues one which did not end normally.
pub(crate) fn continue_note(interrupted: bool, failure: Option<FailureKind>) -> String {
    let how = match failure {
        _ if interrupted => "被中断",
        Some(FailureKind::Quota) => "因额度不足而失败",
        _ => "失败",
    };
    format!("上一个 turn {how}。请先检查当前工作目录的状态，再继续工作。")
}

/// The user or the manager (`by`) sent the candidate back. The reason starts on the first line,
/// which the GUI shows as the message's summary.
pub(crate) fn sent_back(by: &str, reason: &str) -> String {
    format!("{by}退回了候选成果：{reason}\n\n请修改后再次报告 done。")
}
