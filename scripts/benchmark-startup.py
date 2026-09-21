#!/usr/bin/env python3
"""Repeat the real TUI startup in a fixed-size PTY; no playback commands are sent."""
import argparse
import errno
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import pty
import re
import selectors
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import termios
import time

TIMESTAMP = re.compile(r"\+(\d+(?:\.\d+)?)ms .*?mkp_startup: event=(\w+)(.*)")
MILESTONES = ("playback_queue", "sidebar", "visible_view", "complete_view", "background_complete")
STREAMING = {"GetPlaylists", "GetPlaylist", "Search", "GetArtistDetail"}


def field(text, name):
    match = re.search(r"(?:^| )" + re.escape(name) + r"=(Some\(\d+\)|None|[^ ]+)", text)
    return match[1] if match else None


def number(value):
    return int(value[5:-1]) if value and value.startswith("Some(") else None


class Sample:
    def __init__(self):
        self.tls_ms = None
        self.milestones = {}
        self.streaming = set()
        self.requests = {}
        self.error = None
        self.metadata = {}
        self.observation = None
        self.last_draw_ms = None

    def feed(self, line):
        match = TIMESTAMP.search(line)
        if not match:
            return
        elapsed, event, rest = float(match[1]), match[2], match[3]
        if event == "start":
            self.metadata = {k: field(rest, k) for k in ("version", "profile", "os", "arch")}
        elif event == "client_tls_ready":
            if self.tls_ms is not None:
                self.error = "reconnected during startup"
            else:
                self.tls_ms = elapsed
        elif event == "request_queued":
            seq, task, msg = int(field(rest, "seq")), number(field(rest, "task")), field(rest, "msg")
            self.requests[seq] = (msg, task)
            if task is not None and msg in STREAMING:
                self.streaming.add(task)
        elif event == "task_completed":
            # Uncorrelated peer broadcasts can reuse our task numbers.
            task = number(field(rest, "envelope_task"))
            if task is not None and task == int(field(rest, "task")):
                self.streaming.discard(task)
        elif event == "task_failed":
            task = number(field(rest, "envelope_task"))
            if task in self.streaming:
                self.error = "startup task failed: " + rest.strip()
        elif event == "response_error":
            if int(field(rest, "seq")) in self.requests:
                self.error = "startup request failed: " + rest.strip()
        elif event in ("link_closed", "cred_error", "persist_error", "pair_failed"):
            self.error = event + ": " + rest.strip()
        elif event == "probe_result" and field(rest, "ok") == "false":
            self.error = "server probe failed"
        elif event == "link_connect" and field(rest, "kind") == "Pairing":
            self.error = "selected server is not paired"
        elif event == "startup_observation":
            self.observation = json.loads(rest.split("json=", 1)[1])
            self.last_draw_ms = elapsed
            if self.observation["failed"]:
                self.error = "startup displayed an error"
            if self.tls_ms is not None:
                for key in MILESTONES[:-1]:
                    if self.observation[key]:
                        # Full artist/search streams must complete, even if initial rows are present.
                        if key == "complete_view" and self.view_stream_pending():
                            continue
                        self.milestones.setdefault(key, elapsed - self.tls_ms)
                if all(k in self.milestones for k in MILESTONES[:-1]) and not self.streaming:
                    self.milestones.setdefault("background_complete", elapsed - self.tls_ms)

    def view_stream_pending(self):
        return any(task in self.streaming and msg != "GetPlaylists" for msg, task in self.requests.values())

    @property
    def done(self):
        return self.error is not None or all(k in self.milestones for k in MILESTONES)

    def result(self, status, wall_ms):
        return dict(status=status, error=self.error, wall_ms=wall_ms,
                    process_to_tls_ms=self.tls_ms, post_tls_ms=self.milestones,
                    missing=[k for k in MILESTONES if k not in self.milestones],
                    pending_tasks=sorted(self.streaming), build=self.metadata,
                    final_draw=self.observation)


def stop(proc):
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        try:
            proc.wait(timeout=1)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait(timeout=2)


def capture(binary, config, timeout, rows, cols, log_path):
    sample = Sample()
    started = time.monotonic()
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    env = dict(os.environ, MKP_CONFIG_HOME=str(config), RUST_LOG="mkp_startup=trace", TERM="xterm-256color")

    def terminal_session():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    proc = None
    pending = b""
    status = "timeout"
    try:
        proc = subprocess.Popen([str(binary)], stdin=slave, stdout=slave, stderr=subprocess.PIPE,
                                env=env, preexec_fn=terminal_session)
        os.close(slave)
        slave = None
        with selectors.DefaultSelector() as selector, log_path.open("wb") as logfile:
            selector.register(master, selectors.EVENT_READ, "terminal")
            selector.register(proc.stderr, selectors.EVENT_READ, "trace")
            while time.monotonic() - started < timeout:
                for key, _ in selector.select(min(0.1, max(0, timeout - (time.monotonic() - started)))):
                    try:
                        chunk = os.read(key.fd, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        chunk = b""
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    if key.data == "trace":
                        logfile.write(chunk)
                        pending += chunk
                        while b"\n" in pending:
                            line, pending = pending.split(b"\n", 1)
                            sample.feed(line.decode("utf-8", errors="replace"))
                        if len(pending) > 1024 * 1024:
                            raise ValueError("unterminated trace record exceeds 1 MiB")
                if sample.done:
                    status = "failed" if sample.error else "ok"
                    break
                if proc.poll() is not None and not selector.get_map():
                    sample.error = f"client exited before readiness (exit {proc.returncode})"
                    status = "failed"
                    break
            if status == "timeout":
                sample.error = f"startup exceeded {timeout:g}s"
    except (OSError, ValueError, KeyError, TypeError) as error:
        status, sample.error = "failed", str(error)
    finally:
        if proc is not None:
            stop(proc)
            proc.stderr.close()
        os.close(master)
        if slave is not None:
            os.close(slave)
    return sample.result(status, (time.monotonic() - started) * 1000)


def summarize(samples):
    good = [s for s in samples if s["status"] == "ok"]
    metrics = {}
    for key in ("process_to_tls",) + MILESTONES:
        values = sorted(s["process_to_tls_ms"] if key == "process_to_tls" else s["post_tls_ms"][key] for s in good)
        metrics[key] = dict(n=len(values), median_ms=statistics.median(values) if values else None,
                            p95_ms=values[math.ceil(0.95 * len(values)) - 1] if values else None)
    return dict(samples=len(samples), successful=len(good), failed=sum(s["status"] == "failed" for s in samples),
                timed_out=sum(s["status"] == "timeout" for s in samples), metrics=metrics)


def config_home():
    if os.environ.get("MKP_CONFIG_HOME"):
        return Path(os.environ["MKP_CONFIG_HOME"])
    return Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))) / "mkp"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--server", required=True, help="Exact paired server name shown in the picker")
    parser.add_argument("--config", type=Path, default=config_home())
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--scenario", choices=("warm", "restarted", "uncached"), default="warm")
    parser.add_argument("--prepare", type=Path, help="Executable run before EACH sample; required for restarted/uncached scenarios")
    parser.add_argument("--prepare-timeout", type=float, default=60)
    parser.add_argument("--fixture", required=True, help="Description of server build, saved view, and scenario setup")
    parser.add_argument("--rows", type=int, default=40)
    parser.add_argument("--cols", type=int, default=140)
    parser.add_argument("--output", type=Path, required=True, help="New output directory (contains report and per-run TRACE logs)")
    args = parser.parse_args()
    if args.runs < 1 or args.warmup < 0 or not (0 < args.timeout <= 3600) or not (0 < args.prepare_timeout <= 3600) or args.rows < 12 or args.cols < 60:
        parser.error("invalid run count, timeout, or terminal dimensions (minimum 60x12)")
    if args.scenario != "warm" and args.prepare is None:
        parser.error("restarted/uncached scenarios require --prepare to reset the fixture before each run")
    args.binary = args.binary.resolve()
    if not args.binary.is_file() or not args.config.is_dir():
        parser.error("binary and config directory must exist")
    if args.prepare and not os.access(args.prepare.resolve(), os.X_OK):
        parser.error("--prepare must be an executable file")
    if args.output.exists():
        parser.error("--output must be a new directory")
    args.output.mkdir(parents=True, mode=0o700)
    samples, warmups = [], []
    report = dict(schema=1, scenario=args.scenario, fixture=args.fixture, server=args.server,
                  binary=str(args.binary), binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                  terminal=dict(rows=args.rows, cols=args.cols), rust_log="mkp_startup=trace",
                  timeout_s=args.timeout, requested_runs=args.runs, requested_warmups=args.warmup,
                  prepare=str(args.prepare.resolve()) if args.prepare else None,
                  prepare_timeout_s=args.prepare_timeout, samples=samples, warmups=warmups)
    # Snapshot once, then copy per run so cursor/persistence writes cannot change later fixtures.
    with tempfile.TemporaryDirectory(prefix="mkp-benchmark-") as temporary:
        root = Path(temporary)
        baseline = root / "baseline"
        shutil.copytree(args.config, baseline)
        (baseline / "last_server").write_text(args.server + "\n")
        server_dir = args.server.replace("/", "_").replace("\\", "_").replace("\0", "_").lstrip(".")
        saved_view = baseline / server_dir / "last_view"
        report["saved_view_sha256"] = hashlib.sha256(saved_view.read_bytes()).hexdigest() if saved_view.exists() else None
        try:
            for index in range(args.warmup + args.runs):
                warmup = index < args.warmup
                label = f"{'warmup' if warmup else 'run'}-{index + 1:03d}"
                config = root / label
                shutil.copytree(baseline, config)
                result = None
                if args.prepare:
                    try:
                        with (args.output / f"{label}-prepare.log").open("wb") as out:
                            prepared = subprocess.Popen([str(args.prepare.resolve())], stdout=out, stderr=subprocess.STDOUT,
                                                        start_new_session=True)
                            try:
                                code = prepared.wait(timeout=args.prepare_timeout)
                                if code:
                                    raise subprocess.CalledProcessError(code, str(args.prepare))
                            finally:
                                stop(prepared)
                    except (OSError, subprocess.SubprocessError) as error:
                        result = Sample().result("failed", 0)
                        result["error"] = "fixture preparation failed: " + str(error)
                if result is None:
                    result = capture(args.binary, config, args.timeout, args.rows, args.cols, args.output / f"{label}.log")
                result["sample"] = label
                (warmups if warmup else samples).append(result)
                print(f"{label}: {result['status']} {result['error'] or result['post_tls_ms']}", flush=True)
                shutil.rmtree(config)
                report["summary"] = summarize(samples)
                (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        except KeyboardInterrupt:
            report["interrupted"] = True
        finally:
            report["summary"] = summarize(samples)
            (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    summary = report["summary"]
    lines = [f"{args.scenario}: {summary['successful']}/{summary['samples']} successful; {summary['failed']} failed; {summary['timed_out']} timed out",
             "Milliseconds after TLS, except process_to_tls. Percentiles use successful full runs only; failures are reported separately."]
    for key, metric in summary["metrics"].items():
        median = f"{metric['median_ms']:.3f}" if metric['median_ms'] is not None else "n/a"
        p95 = f"{metric['p95_ms']:.3f}" if metric['p95_ms'] is not None else "n/a"
        lines.append(f"{key}: n={metric['n']} median={median} p95={p95}")
    (args.output / "summary.txt").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0 if len(samples) == args.runs and all(s["status"] == "ok" for s in samples + warmups) else 1


if __name__ == "__main__":
    sys.exit(main())
