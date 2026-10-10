//! What the runtime tells an executor, in the words it reads (#16). The commands decide when to say
//! something; this module says it, so the wording can be read and changed in one place, and can
//! later differ by harness. The GUI reads two shapes back: a message's header (`header`) and the
//! opening of the runtime's note (frontend `continueNote`).

use crate::capture::{NewFile, UncoveredPath};
use crate::turn::FailureKind;

/// How many paths a note lists before it summarizes.
const LISTED: usize = 50;

fn bullets(lines: impl IntoIterator<Item = String>) -> String {
    lines.into_iter().map(|line| format!("- {line}")).collect::<Vec<_>>().join("\n")
}

/// The task as the executor first sees it: the user's words and the current criteria. The first
/// line doubles as its summary in the GUI.
pub(crate) fn brief(title: &str, body: &str, seq: i64, version: i64, criteria: &str) -> String {
    format!(
        "任务：{title}（第 {seq} 轮执行）\n\n\
         用户原话：\n{body}\n\n\
         完成条件（第 {version} 版）：\n{criteria}\n\n\
         完成后调用 org_report，status 为 done。若有适合用户体验成果的命令，可附带 trial：\
         command 是从候选成果根目录运行的具体命令，Windows 上由 Windows PowerShell 执行，\
         Linux 上由 sh 执行；purpose 说明体验目的。\
         trial 只提供建议，用户明确点击后才会运行；它不会自动执行，也不属于自动验证检查。\
         遇到需要用户决定的问题时，调用 org_report，status 为 blocked，\
         并在 blocked_on 中写明问题。"
    )
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

/// The user sent the candidate back. The reason starts on the first line, which the GUI shows as
/// the message's summary.
pub(crate) fn sent_back(reason: &str) -> String {
    format!("用户退回了候选成果：{reason}\n\n请修改后再次报告 done。")
}
