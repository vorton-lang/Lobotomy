"""Temporary, bounded same-runner experiment. No environment or raw test-output upload."""
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
BACKEND = ROOT / "backend"
STORE = BACKEND / "crates/store"
OUT = Path(os.environ.get("LOBOTOMY_EXPERIMENT_OUT", ROOT / "git-experiment-results"))
STATE = OUT / "result.json"
TESTS = ["a_slot_is_a_git_clone_at_the_baseline", "a_capture_is_the_working_tree_on_the_baseline"]


def save(data):
    OUT.mkdir(parents=True, exist_ok=True)
    STATE.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def build():
    head = git("rev-parse", "HEAD")
    assert head == os.environ["LOBOTOMY_EXPERIMENT_HEAD"], "checkout must be exact PR head"
    assert not git("status", "--porcelain", "--untracked-files=no"), "tracked source must be clean"
    data = {"head": head, "base": os.environ.get("LOBOTOMY_EXPERIMENT_BASE", "local"), "status": "building", "samples": [],
            "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "runner_os": os.environ.get("RUNNER_OS", sys.platform),
            "image_version": os.environ.get("ImageVersion", "local"),
            "build_command": "cargo test --workspace --locked --no-run --message-format=json"}
    save(data)
    start = time.perf_counter()
    proc = subprocess.run(data["build_command"].split(), cwd=BACKEND, stdout=subprocess.PIPE, text=True)
    data["build_seconds"] = time.perf_counter() - start
    data["build_exit_code"] = proc.returncode
    save(data)
    assert proc.returncode == 0, "build failed (see compiler log)"
    binaries = []
    for line in proc.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact" and message.get("target", {}).get("name") == "store"
                and message.get("profile", {}).get("test") and message.get("executable")):
            binaries.append(message["executable"])
    assert len(binaries) == 1, f"expected one store integration-test binary, got {len(binaries)}"
    data.update(binary=binaries[0], binary_sha256=sha256(binaries[0]), status="built")
    save(data)
    print(f"Exact head {head}; build {data['build_seconds']:.3f}s; store binary SHA256 {data['binary_sha256']}")


def run_test(binary, name, env, ignored=False):
    args = [binary, name, "--exact", "--test-threads=1", "--nocapture"]
    if ignored:
        args.append("--ignored")
    start = time.perf_counter()
    proc = subprocess.run(args, cwd=STORE, env=env, text=True, capture_output=True, timeout=120)
    elapsed = (time.perf_counter() - start) * 1000
    # Never mistake a typo / zero selected tests for a passing sample.
    assert proc.returncode == 0 and "test result: ok. 1 passed; 0 failed;" in proc.stdout, (
        f"{name}: exit={proc.returncode}; expected exactly one passing test; raw output intentionally not uploaded")
    records = {}
    for line in proc.stdout.splitlines():
        if line.startswith("GIT_EXPERIMENT\t"):
            _, key, value = line.split("\t", 2)
            assert key not in records, f"duplicate diagnostic key {key}"
            records[key] = value
    return elapsed, records


def probe(binary, env):
    _, result = run_test(binary, "git_entry_experiment::probe", env, True)
    assert set(result) == {"image_fixture", "image_store", "version"}
    for key in ("image_fixture", "image_store"):
        assert Path(result[key]).is_absolute() and Path(result[key]).is_file()
    return result


def measure():
    data = json.loads(STATE.read_text(encoding="utf-8"))
    try:
        binary = data["binary"]
        assert git("rev-parse", "HEAD") == data["head"]
        assert sha256(binary) == data["binary_sha256"], "test binary changed after build"
        assert not git("status", "--porcelain", "--untracked-files=no")
        env_a = os.environ.copy()
        env_a["LOBOTOMY_GIT_EXPERIMENT"] = "1"
        assert "LOBOTOMY_EXPERIMENT_PROGRAM" not in env_a
        initial = probe(binary, env_a)
        data["initial"] = initial
        save(data)
        if os.name == "nt":
            pairs = [(p / "cmd/git.exe", p / "mingw64/bin/git.exe")
                     for p in Path(initial["image_fixture"]).parents]
            pairs = [(cmd, core) for cmd, core in pairs if cmd.is_file() and core.is_file()]
            assert len(pairs) == 1, "cannot uniquely locate cmd/core from actual image; no guessed comparison"
            cmd, core = pairs[0]
        else:
            # Exercise orchestration locally without claiming Windows or wrapper/core evidence.
            cmd = core = Path(initial["image_fixture"])
        data["candidates"] = {}
        for label, program in (("cmd", cmd), ("core", core)):
            candidate_env = dict(env_a, LOBOTOMY_EXPERIMENT_PROGRAM=str(program))
            result = probe(binary, candidate_env)
            assert result["version"] == initial["version"], "Git version mismatch"
            assert os.path.samefile(result["image_fixture"], program)
            result["sha256"] = sha256(program)
            data["candidates"][label] = result
            save(data)
        env_b = dict(env_a, PATH=str(core.parent) + os.pathsep + env_a["PATH"])
        data["default_is_core"] = os.path.samefile(initial["image_fixture"], core)
        data["method"] = {"order": list("ABBAABBA"), "version_calls_per_block": 10,
                          "tests": TESTS, "warmup": "one unmeasured A and B block",
                          "A": "unaltered inherited PATH", "B": "only prepend core directory to child PATH",
                          "store_ms": "external wall time: process start, fixture, assertions, cleanup, exit",
                          "version_ms": "Rust Instant around Command::new(git).arg(--version).output()"}
        data["status"] = "measuring"
        save(data)
        for index, mode in enumerate("AB" + "ABBAABBA"):
            env = env_a if mode == "A" else env_b
            entry = probe(binary, env)
            expected = initial["image_fixture"] if mode == "A" else str(core)
            assert entry["version"] == initial["version"]
            for key in ("image_fixture", "image_store"):
                assert os.path.samefile(entry[key], expected), "PATH did not select the expected actual image"
            _, values = run_test(binary, "git_entry_experiment::version_bench", env, True)
            assert len(values) == 10
            version_ms = [float(values[f"version_ms_{i}"]) for i in range(10)]
            tests = {name: run_test(binary, name, env)[0] for name in TESTS}
            row = {"block": index - 1, "mode": mode, "warmup": index < 2, "entry": entry,
                   "version_ms": version_ms, "store_ms": tests, "result": "all selected tests passed"}
            data["samples"].append(row)
            save(data)
            print(f"{'warmup' if index < 2 else 'sample'} {index - 1} {mode}: "
                  f"version median {statistics.median(version_ms):.3f} ms; "
                  f"store {[round(v, 3) for v in tests.values()]} ms", flush=True)
        restored = probe(binary, env_a)
        assert restored == initial, "default entry/version not restored"
        assert os.environ["PATH"] == env_a["PATH"], "parent PATH changed"
        assert sha256(binary) == data["binary_sha256"], "binary changed during measurements"
        data.update(restored=restored, status="complete")
        save(data)
        summarize(data)
    except BaseException as error:
        data.update(status="failed", error=f"{type(error).__name__}: {error}")
        save(data)
        raise


def summarize(data):
    rows = [row for row in data["samples"] if not row["warmup"]]
    lines = ["## Temporary Windows Git entry experiment", "",
             f"Exact head: `{data['head']}`. Build: {data['build_seconds']:.3f} s.", "",
             f"Default: `{data['initial']['image_fixture']}`.",
             f"Core: `{data['candidates']['core']['image_fixture']}`.",
             f"Version: `{data['initial']['version']}`. Default already core: `{data['default_is_core']}`.", "",
             "| Metric (ms) | A median | B median | A − B | Reduction | A / B |", "|---|---:|---:|---:|---:|---:|"]
    for metric in ["git --version", *TESTS]:
        medians = {}
        for mode in "AB":
            selected = [row for row in rows if row["mode"] == mode]
            values = ([statistics.median(row["version_ms"]) for row in selected] if metric == "git --version"
                      else [row["store_ms"][metric] for row in selected])
            medians[mode] = statistics.median(values)
        a, b = medians["A"], medians["B"]
        lines.append(f"| {metric} | {a:.3f} | {b:.3f} | {a-b:.3f} | {(a-b)/a:.1%} | {a/b:.3f}× |")
    lines += ["", "Two ABBA blocks after one warmup per mode; four measured blocks per mode. "
              "Version metric is the median of four block medians (10 calls/block). "
              "Store times include fixture setup, assertions, cleanup and process launch. "
              "Same binary and runner; only the measured child's PATH changes. "
              "Small sequential sample, no whole-suite or cross-platform speedup claim. "
              "Suspended probes record the initial image, not the wrapper's descendant process tree. "
              "Normal CI gates still apply; no retries or security-setting changes.", ""]
    summary = "\n".join(lines)
    (OUT / "summary.md").write_text(summary, encoding="utf-8")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as output:
            output.write(summary)
    print(summary)


if __name__ == "__main__":
    {"build": build, "measure": measure}[sys.argv[1]]()
