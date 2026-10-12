// The settings (frontend.md §7.8): a page of their own, in place of the window's body while open.
// They choose their page in their own navigation, whatever project the window shows: the host's
// permissions, Angela, and two pages per project. Following GitHub Primer's settings patterns, a
// page either applies each choice at once (the segmented controls) or saves a form explicitly
// (the project configuration), never both; archiving sits apart at the bottom.

import { useEffect, useState, type ReactNode } from 'react';
import { ArrowLeft, X } from 'lucide-react';
import type { Check, HarnessView, Permission, ProjectConfig, RoleView } from '../api/types';
import { bytes, harnessName, PERMISSION_LABEL } from '../format';
import {
  act,
  angelaOpen,
  call,
  closeSettings,
  runIn,
  setPermission,
  showSettingsPage,
  toast,
  useStore,
  type SettingsPage,
} from '../store';
import { ICON } from './common';

export function Settings() {
  const settings = useStore((s) => s.settings);
  const host = useStore((s) => s.host);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && closeSettings();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
  if (!settings || !host) return null;
  const open = host.projects.filter((p) => p.state === 'running' && p.error === null);
  const page = settings.page;
  const is = (other: SettingsPage) => JSON.stringify(other) === JSON.stringify(page);
  const item = (target: SettingsPage, label: string) => (
    <button className={`settings-item ${is(target) ? 'selected' : ''}`} aria-current={is(target) || undefined} onClick={() => showSettingsPage(target)}>
      {label}
    </button>
  );
  return (
    <section className="settings-page" aria-label="设置">
      <nav className="settings-nav" aria-label="设置分类">
        <button className="ghost settings-back with-icon" onClick={closeSettings}>
          <ArrowLeft {...ICON} />
          返回
        </button>
        <h3>本机</h3>
        {item({ kind: 'permissions' }, '权限')}
        {angelaOpen(host) && item({ kind: 'angela' }, 'Angela')}
        {open.map((p) => (
          <div key={p.id} className="settings-nav-group">
            <h3 title={p.repo_path}>{p.name}</h3>
            {item({ kind: 'project', project: p.id, tab: 'general' }, '常规')}
            {item({ kind: 'project', project: p.id, tab: 'checks' }, '验证与采集')}
          </div>
        ))}
      </nav>
      <div className="settings-content">
        {page.kind === 'permissions' && <Permissions harnesses={host.harnesses} />}
        {page.kind === 'angela' && <AngelaPage />}
        {page.kind === 'project' && page.tab === 'general' && <ProjectGeneral project={page.project} />}
        {page.kind === 'project' && page.tab === 'checks' && <ProjectChecks project={page.project} />}
      </div>
    </section>
  );
}

function Header({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <header className="settings-header">
      <h2>{title}</h2>
      {children && <p className="muted">{children}</p>}
    </header>
  );
}

/** One setting: what it is on the left, its control on the right. */
function Row({ label, description, children }: { label: ReactNode; description?: ReactNode; children?: ReactNode }) {
  return (
    <div className="setting-row">
      <div className="setting-text">
        <div className="setting-label">{label}</div>
        {description && <div className="setting-description muted">{description}</div>}
      </div>
      {children && <div className="setting-control">{children}</div>}
    </div>
  );
}

/** Two or three choices that apply at once. */
function Segmented<T extends string>({
  name,
  label,
  value,
  options,
  disabled,
  onChange,
}: {
  name: string;
  label: string;
  value: T;
  options: { value: T; label: string }[];
  disabled?: boolean;
  onChange: (value: T) => void;
}) {
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <label key={o.value} className={o.value === value ? 'selected' : ''}>
          <input type="radio" name={name} checked={o.value === value} disabled={disabled} onChange={() => onChange(o.value)} />
          {o.label}
        </label>
      ))}
    </div>
  );
}

const permissionNote = (permission: Permission, harness: string) =>
  permission === 'full' ? '不审批，不用沙箱。' : `在沙箱中工作，越界的操作由 ${harness} 自动审核。管理员不允许完全放开时用这一项。`;

/** Each harness's permission mode, for every project (harness-adapter.md §1.9). */
function Permissions({ harnesses }: { harnesses: HarnessView[] }) {
  return (
    <>
      <Header title="权限">本机所有项目共用，下一个 turn 生效。</Header>
      {harnesses.map((h) => {
        const name = harnessName(h.harness);
        return (
          <Row key={h.harness} label={name} description={permissionNote(h.permission, name)}>
            <Segmented
              name={`permission-${h.harness}`}
              label={`${name} 的权限`}
              value={h.permission}
              options={(['full', 'auto_review'] as const).map((p) => ({ value: p, label: PERMISSION_LABEL[p] }))}
              onChange={(p) => setPermission(h.harness, p)}
            />
          </Row>
        );
      })}
    </>
  );
}

/** The snapshot of the page's project or of Angela, loaded by the settings themselves. */
function usePageSnapshot() {
  return useStore((s) => s.settings?.snapshot ?? null);
}

/**
 * A role's harness, and its model and reasoning effort there (harness-adapter.md §0, §1.2). No
 * role is bound to a harness. Each harness keeps the role's own model choice; "默认" leaves it to
 * the CLI.
 */
function RoleRuntime({ target, role, harnesses }: { target: string; role: RoleView; harnesses: HarnessView[] }) {
  const running = role.unfinished !== null;
  const harness = harnesses.find((h) => h.harness === role.harness);
  const description = running
    ? 'turn 运行中，结束后才能换 harness。模型和推理强度从下一个 turn 起生效。'
    : '换 harness 会开新会话。换模型或推理强度从下一个 turn 起生效，会话不变。';
  return (
    <Row label={role.name} description={description}>
      <div className="role-runtime">
        <Segmented
          name={`harness-${target}-${role.name}`}
          label={`${role.name} 使用的 harness`}
          value={role.harness}
          options={harnesses.map((h) => ({ value: h.harness, label: harnessName(h.harness) }))}
          disabled={running}
          onChange={(harness) => runIn(target, 'set_role_harness', { role: role.name, harness })}
        />
        {harness && <ModelChoice target={target} role={role} harness={harness} />}
      </div>
    </Row>
  );
}

/** The model and the effort, as the harness lists them; typed in when it lists no models. */
function ModelChoice({ target, role, harness }: { target: string; role: RoleView; harness: HarnessView }) {
  const name = harnessName(harness.harness);
  const choose = (model: string | null, effort: string | null) =>
    runIn(target, 'set_role_model', { role: role.name, harness: harness.harness, model, effort });
  if (harness.models.length === 0) {
    return (
      <>
        <Typed label={`${role.name} 在 ${name} 上的模型`} placeholder="默认模型" value={role.model} onCommit={(model) => choose(model, role.effort)} />
        <Typed label={`${role.name} 在 ${name} 上的推理强度`} placeholder="默认强度" value={role.effort} onCommit={(effort) => choose(role.model, effort)} />
      </>
    );
  }
  const model = harness.models.find((m) => m.id === role.model);
  const effortsOf = (id: string | null) => harness.models.find((m) => m.id === id)?.efforts ?? harness.efforts;
  const efforts = effortsOf(role.model);
  return (
    <>
      <select
        aria-label={`${role.name} 在 ${name} 上的模型`}
        value={role.model ?? ''}
        onChange={(e) => {
          const next = e.target.value || null;
          // An effort the new model does not take goes back to its default.
          void choose(next, role.effort !== null && effortsOf(next).includes(role.effort) ? role.effort : null);
        }}
      >
        <option value="">默认模型</option>
        {harness.models.map((m) => (
          <option key={m.id} value={m.id}>
            {m.label}
          </option>
        ))}
        {role.model !== null && !model && <option value={role.model}>{role.model}</option>}
      </select>
      <select aria-label={`${role.name} 在 ${name} 上的推理强度`} value={role.effort ?? ''} onChange={(e) => void choose(role.model, e.target.value || null)}>
        <option value="">{model?.default_effort ? `默认（${model.default_effort}）` : '默认强度'}</option>
        {efforts.map((effort) => (
          <option key={effort} value={effort}>
            {effort}
          </option>
        ))}
        {role.effort !== null && !efforts.includes(role.effort) && <option value={role.effort}>{role.effort}</option>}
      </select>
    </>
  );
}

/** A name typed in, saved when the field loses focus or on Enter; empty means the default. */
function Typed({ label, placeholder, value, onCommit }: { label: string; placeholder: string; value: string | null; onCommit: (value: string | null) => void }) {
  const [text, setText] = useState(value ?? '');
  useEffect(() => setText(value ?? ''), [value]);
  const commit = () => {
    const next = text.trim() || null;
    if (next !== value) onCommit(next);
  };
  return (
    <input
      className="typed-model"
      aria-label={label}
      placeholder={placeholder}
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => e.key === 'Enter' && commit()}
    />
  );
}

function AngelaPage() {
  const snapshot = usePageSnapshot();
  const harnesses = useStore((s) => s.host?.harnesses) ?? [];
  const role = snapshot?.roles[0];
  return (
    <>
      <Header title="Angela">全局协作者，所有项目共用。</Header>
      {role && <RoleRuntime target="angela" role={role} harnesses={harnesses} />}
    </>
  );
}

function ProjectGeneral({ project }: { project: string }) {
  const snapshot = usePageSnapshot();
  const name = useStore((s) => s.host?.projects.find((p) => p.id === project)?.name ?? '');
  const harnesses = useStore((s) => s.host?.harnesses) ?? [];
  return (
    <>
      <Header title={name}>{snapshot?.project?.repo_path}</Header>
      <h3 className="settings-section">角色</h3>
      {snapshot?.roles.map((role) => (
        <RoleRuntime key={role.name} target={project} role={role} harnesses={harnesses} />
      ))}
      <h3 className="settings-section">存储</h3>
      <Storage project={project} />
      <Archive project={project} name={name} />
    </>
  );
}

/** What the raw output of turns takes on disk (data-model.md §7.4); read when the page opens. */
function Storage({ project }: { project: string }) {
  const [usage, setUsage] = useState<{ files: number; bytes: number } | null>(null);
  useEffect(() => {
    call('disk_usage', { project }).then(
      (u) => setUsage(u.raw_output),
      () => setUsage(null),
    );
  }, [project]);
  return (
    <Row label="保留的原始输出" description="turn 没有正常结束时保留，用于排查；不自动删除。">
      <span className="muted">{usage ? `${usage.files} 个文件 · ${bytes(usage.bytes)}` : '…'}</span>
    </Row>
  );
}

/**
 * Archiving stops the project as quitting does and keeps its data (data-model.md §10.4). It
 * interrupts running turns, so it asks once more.
 */
function Archive({ project, name }: { project: string; name: string }) {
  const [confirming, setConfirming] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const archive = async () => {
    setArchiving(true);
    await act('archive_project', { project_id: project });
    setArchiving(false);
    setConfirming(false);
  };
  return (
    <div className="danger-zone">
      <Row label="归档这个项目" description="停止调度，中断正在运行的 turn。数据保留，可以取消归档。">
        {confirming ? (
          <>
            <button onClick={() => setConfirming(false)} disabled={archiving}>
              取消
            </button>
            <button className="danger" onClick={archive} disabled={archiving}>
              {archiving ? '正在归档…' : `归档「${name}」`}
            </button>
          </>
        ) : (
          <button className="danger" onClick={() => setConfirming(true)}>
            归档…
          </button>
        )}
      </Row>
    </div>
  );
}

const lines = (text: string) =>
  text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);

/** The form's fields, as the configuration fills them. */
interface Form {
  checks: Check[];
  excluded: string;
  forced: string;
  maxFiles: number;
  maxMb: number;
}

const formOf = (config: ProjectConfig): Form => ({
  checks: config.checks,
  excluded: config.excluded.join('\n'),
  forced: config.force_tracked.join('\n'),
  maxFiles: config.max_new_files,
  maxMb: Math.round(config.max_new_bytes / 1024 / 1024),
});

const configOf = (form: Form): ProjectConfig => ({
  checks: form.checks.filter((c) => c.command.trim()),
  excluded: lines(form.excluded),
  force_tracked: lines(form.forced),
  max_new_files: form.maxFiles,
  max_new_bytes: form.maxMb * 1024 * 1024,
});

/**
 * The project configuration (data-model.md §9.1 project_config): each save is a new version, and
 * checks and captures name the version they used. The form saves the version it was filled from;
 * one saved meanwhile in another window makes the backend refuse, not overwrite (#16).
 */
function ProjectChecks({ project }: { project: string }) {
  const current = useStore((s) => s.settings?.snapshot?.config ?? null);
  if (!current) return <Header title="验证与采集" />;
  return <ConfigForm key={project} project={project} version={current.version} config={current.config} />;
}

function ConfigForm({ project, version, config }: { project: string; version: number; config: ProjectConfig }) {
  const [base, setBase] = useState({ version, config });
  const [form, setForm] = useState(() => formOf(config));
  const [saving, setSaving] = useState(false);
  const dirty = JSON.stringify(configOf(form)) !== JSON.stringify(configOf(formOf(base.config)));
  // A newer version saved elsewhere replaces an untouched form; an edited one keeps its edits. The
  // snapshot may still name an older version for a moment after this form saved.
  const newer = version > base.version;
  useEffect(() => {
    if (newer && !dirty) {
      setBase({ version, config });
      setForm(formOf(config));
    }
  }, [newer, version, config, dirty]);
  const set = (patch: Partial<Form>) => setForm({ ...form, ...patch });
  const setCheck = (i: number, patch: Partial<Check>) => set({ checks: form.checks.map((c, j) => (i === j ? { ...c, ...patch } : c)) });
  const reset = () => {
    setBase({ version, config });
    setForm(formOf(config));
  };
  const save = async () => {
    setSaving(true);
    const saved = await runIn(project, 'edit_project_config', { expected_version: base.version, config: configOf(form) });
    setSaving(false);
    if (saved === undefined) return;
    toast(`已保存为第 ${saved.version} 版`);
    const next = configOf(form);
    setBase({ version: saved.version, config: next });
    setForm(formOf(next));
  };

  return (
    <form
      className="settings-form"
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <Header title="验证与采集">候选成果在验证现场按顺序运行检查命令，全部退出码为 0 才算通过。</Header>
      <h3 className="settings-section">检查命令</h3>
      {form.checks.length === 0 && <p className="muted">没有检查命令：候选成果直接进入验收。</p>}
      {form.checks.map((check, i) => (
        <div key={i} className="check-row">
          <input value={check.command} onChange={(e) => setCheck(i, { command: e.target.value })} placeholder="例如 cargo test" aria-label="检查命令" />
          <input type="number" min={1} value={check.timeout_secs} onChange={(e) => setCheck(i, { timeout_secs: Number(e.target.value) })} aria-label="超时（秒）" />
          <span className="muted">秒</span>
          <button type="button" className="icon" onClick={() => set({ checks: form.checks.filter((_, j) => j !== i) })} aria-label="删除">
            <X {...ICON} />
          </button>
        </div>
      ))}
      <button type="button" className="small" onClick={() => set({ checks: [...form.checks, { command: '', timeout_secs: 600 }] })}>
        + 添加检查命令
      </button>

      <h3 className="settings-section">采集范围</h3>
      <label>
        始终排除（gitignore 写法，每行一条）
        <textarea value={form.excluded} onChange={(e) => set({ excluded: e.target.value })} rows={3} placeholder={'target/\nnode_modules/'} />
      </label>
      <label>
        强制采集（路径前缀，每行一条）
        <textarea value={form.forced} onChange={(e) => set({ forced: e.target.value })} rows={2} />
      </label>

      <h3 className="settings-section">体积护栏</h3>
      <Row label="一次采集新增的上限" description="超过任一上限时采集停下，等你决定。">
        <input type="number" min={0} value={form.maxFiles} onChange={(e) => set({ maxFiles: Number(e.target.value) })} aria-label="新增文件数上限" />
        <span className="muted">个文件</span>
        <input type="number" min={0} value={form.maxMb} onChange={(e) => set({ maxMb: Number(e.target.value) })} aria-label="新增大小上限" />
        <span className="muted">MB</span>
      </Row>

      <footer className="settings-save">
        {newer ? (
          <span className="error">另一个窗口已保存第 {version} 版。放弃修改后在新版本上改。</span>
        ) : (
          <span className="muted">{dirty ? '有未保存的修改' : `第 ${base.version} 版`}</span>
        )}
        <span className="spacer" />
        {(dirty || newer) && (
          <button type="button" onClick={reset}>
            放弃修改
          </button>
        )}
        <button type="submit" className="primary" disabled={saving}>
          {saving ? '正在保存…' : '保存'}
        </button>
      </footer>
    </form>
  );
}
