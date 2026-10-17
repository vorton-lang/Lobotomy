// The settings (frontend.md §7.8): a page of their own, in place of the window's body while open.
// They choose their page in their own navigation, whatever project the window shows: the host's
// permissions, Angela, and a page per project. Following GitHub Primer's settings patterns, a page
// applies each choice at once (the segmented controls); archiving sits apart at the bottom.

import { useEffect, useState, type ReactNode } from 'react';
import { ArrowLeft } from 'lucide-react';
import type { HarnessView, Permission, RoleView } from '../api/types';
import { bytes, harnessName, PERMISSION_LABEL } from '../format';
import {
  act,
  angelaOpen,
  call,
  closeSettings,
  runIn,
  setPermission,
  showSettingsPage,
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
    <button
      className={`settings-item ${is(target) ? 'selected' : ''}`}
      aria-current={is(target) || undefined}
      onClick={() => showSettingsPage(target)}
    >
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
            {item({ kind: 'project', project: p.id }, '常规')}
          </div>
        ))}
      </nav>
      <div className="settings-content">
        {page.kind === 'permissions' && <Permissions harnesses={host.harnesses} />}
        {page.kind === 'angela' && <AngelaPage />}
        {page.kind === 'project' && <ProjectGeneral project={page.project} />}
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
          <input
            type="radio"
            name={name}
            checked={o.value === value}
            disabled={disabled}
            onChange={() => onChange(o.value)}
          />
          {o.label}
        </label>
      ))}
    </div>
  );
}

const permissionNote = (permission: Permission, harness: string) =>
  permission === 'full'
    ? '不审批，不用沙箱。'
    : `在沙箱中工作，越界的操作由 ${harness} 自动审核。管理员不允许完全放开时用这一项。`;

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
        <Typed
          label={`${role.name} 在 ${name} 上的模型`}
          placeholder="默认模型"
          value={role.model}
          onCommit={(model) => choose(model, role.effort)}
        />
        <Typed
          label={`${role.name} 在 ${name} 上的推理强度`}
          placeholder="默认强度"
          value={role.effort}
          onCommit={(effort) => choose(role.model, effort)}
        />
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
      <select
        aria-label={`${role.name} 在 ${name} 上的推理强度`}
        value={role.effort ?? ''}
        onChange={(e) => void choose(role.model, e.target.value || null)}
      >
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
function Typed({
  label,
  placeholder,
  value,
  onCommit,
}: {
  label: string;
  placeholder: string;
  value: string | null;
  onCommit: (value: string | null) => void;
}) {
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

/** The project's workers have their harness and model in `workers.toml`, not here (tech-plan.md §7). */
function ProjectGeneral({ project }: { project: string }) {
  const snapshot = usePageSnapshot();
  const name = useStore((s) => s.host?.projects.find((p) => p.id === project)?.name ?? '');
  return (
    <>
      <Header title={name}>{snapshot?.project?.repo_path}</Header>
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
