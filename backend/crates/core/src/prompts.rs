//! What the runtime tells an executor, in the words it reads (#16). The commands decide when to say
//! something; this module says it, so the wording can be read and changed in one place, and can
//! later differ by harness. The GUI reads two shapes back: a message's header (`header`) and the
//! opening of the runtime's note (frontend `continueNote`).

use crate::capture::{NewFile, UncoveredPath};
use crate::task::Assignment;
use crate::turn::FailureKind;

/// How many paths a note lists before it summarizes.
const LISTED: usize = 50;

/// The manager's name, as messages and briefs name her (roles-and-tasks.md §1.1).
pub const MANAGER: &str = "Angela";

fn bullets(lines: impl IntoIterator<Item = String>) -> String {
    lines.into_iter().map(|line| format!("- {line}")).collect::<Vec<_>>().join("\n")
}

const HOW_TO_REPORT: &str = "完成后调用 org_report，status 为 done。若有适合用户体验成果的命令，可附带 trial：\
     command 是从候选成果根目录运行的具体命令，Windows 上由 Windows PowerShell 执行，\
     Linux 上由 sh 执行；purpose 说明体验目的。\
     trial 只提供建议，用户明确点击后才会运行；它不会自动执行，也不属于自动验证检查。\
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

/// Added to the brief of a task made from changes outside any task (#14).
pub(crate) const STARTS_FROM_OUTSIDE_CHANGES: &str =
    "工作目录里已经有这项任务的初始改动：它们是在任务之外做的，用户把它们建成了这项任务。请在此基础上继续。";

/// Added to the brief when the attempt's code start has conflict markers (data-model.md §4.6).
pub(crate) fn conflicts_at_start(paths: &[String]) -> String {
    format!(
        "这项任务已有的改动与当前的集成版本冲突。工作目录中以下文件有冲突标记，请先解决：\n{}",
        bullets(paths.iter().cloned())
    )
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

/// The runtime's note at the start of a turn that continues one which did not end normally.
pub(crate) fn continue_note(interrupted: bool, failure: Option<FailureKind>) -> String {
    let how = match failure {
        _ if interrupted => "被中断",
        Some(FailureKind::Quota) => "因额度不足而失败",
        _ => "失败",
    };
    format!("上一个 turn {how}。请先检查当前工作目录的状态，再继续工作。")
}

/// What the executor is told about the new files that stopped a capture.
pub(crate) fn capture_oversized(files: &[NewFile], total_bytes: &serde_json::Value) -> String {
    let mut list: Vec<String> = files.iter().take(LISTED).map(|f| format!("{}（{} 字节）", f.path, f.size)).collect();
    if files.len() > LISTED {
        list.push(format!("……另有 {} 个文件", files.len() - LISTED));
    }
    format!(
        "上一个 turn 结束后，采集新增了 {} 个文件，共 {} 字节，超过了项目设定的上限，运行时没有采集。新增的文件：\n{}\n\n\
         依赖、构建输出、缓存这类不该进入成果的文件，请加入 .gitignore 或删除。",
        files.len(),
        total_bytes,
        bullets(list)
    )
}

/// What the executor is told about the content that stopped a capture.
pub(crate) fn capture_uncovered(paths: &[UncoveredPath]) -> String {
    let list = paths.iter().take(LISTED).map(|p| {
        let what = match p.kind.as_str() {
            "nested_repo" => "嵌套的仓库",
            "special_file" => "特殊文件",
            _ => "无法表示的文件名",
        };
        format!("{}（{what}）", p.path)
    });
    format!(
        "上一个 turn 结束后，工作目录里有采集无法保存的内容，运行时没有采集：\n{}\n\n\
         嵌套的仓库请删除其中的 .git，或把整个目录加入 .gitignore；特殊文件请删除。",
        bullets(list)
    )
}

/// Added when the stopped capture was the candidate of a `done`.
pub(crate) fn done_not_taken(note: &str) -> String {
    format!("{note}\n\n你报告的 done 因此没有生效。处理后请再次调用 org_report 报告 done。")
}

/// The candidate conflicts with the integration version it was rebased onto.
pub(crate) fn verification_conflicts(paths: &[String]) -> String {
    format!(
        "集成版本在你开始这项任务之后有了新的提交，你的候选成果与它冲突。工作目录已更新为合并后的结果，HEAD \
         指向新的集成版本。以下文件中有冲突标记：\n{}\n\n请解决冲突后再次报告 done。",
        bullets(paths.iter().cloned())
    )
}

const MOVED: &str =
    "\n\n集成版本在你开始这项任务之后有了新的提交，工作目录已更新为基于它的结果，HEAD 指向新的集成版本。";

/// A check failed. `moved`: the slot now holds the candidate rebased onto a newer version.
pub(crate) fn verification_failed(
    command: &str,
    timed_out: bool,
    exit_code: Option<i64>,
    tail: &str,
    moved: bool,
) -> String {
    let why = match (timed_out, exit_code) {
        (true, _) => "超时".to_owned(),
        (false, Some(code)) => format!("退出码为 {code}"),
        (false, None) => "被终止".to_owned(),
    };
    let moved = if moved { MOVED } else { "" };
    format!(
        "候选成果没有通过验证：检查命令 `{command}` {why}。输出的最后部分：\n```\n{tail}\n```{moved}\n\n请修复后再次报告 done。"
    )
}

/// Not every configured check ran.
pub(crate) fn verification_incomplete(moved: bool) -> String {
    let moved = if moved { MOVED } else { "" };
    format!("候选成果没有通过验证：检查命令没有全部运行。{moved}\n\n请再次报告 done。")
}

/// The user or the manager (`by`) sent the candidate back. The reason starts on the first line,
/// which the GUI shows as the message's summary.
pub(crate) fn sent_back(by: &str, reason: &str) -> String {
    format!("{by}退回了候选成果：{reason}\n\n请修改后再次报告 done。")
}
