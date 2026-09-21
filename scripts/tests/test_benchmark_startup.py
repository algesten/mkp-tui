import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("benchmark_startup", Path(__file__).parents[1] / "benchmark-startup.py")
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def event(ms, name, rest=""):
    return f"2026-01-01T00:00:00Z +{ms:.3f}ms TRACE [main] mkp_startup: event={name} {rest}\n"


def observation(ms, **overrides):
    state = dict(playback_queue=True, sidebar=True, visible_view=True, complete_view=True, failed=False)
    state.update(overrides)
    return event(ms, "startup_observation", "json=" + json.dumps(state))


class Parsing(unittest.TestCase):
    def test_tls_baseline_ignores_probe_and_requires_a_draw_after_completion(self):
        s = bench.Sample()
        s.feed(event(20, "tls_handshake_done"))
        s.feed(observation(25))
        self.assertFalse(s.milestones)
        s.feed(event(100, "client_tls_ready"))
        s.feed(event(101, "request_queued", "seq=3 task=Some(1) msg=GetPlaylists depth=1"))
        s.feed(event(102, "request_queued", "seq=4 task=Some(2) msg=GetPlaylist depth=1"))
        s.feed(observation(110))
        self.assertEqual(s.milestones["visible_view"], 10)
        self.assertNotIn("complete_view", s.milestones)
        s.feed(event(111, "task_completed", "task=2 envelope_task=None"))
        self.assertIn(2, s.streaming)
        s.feed(event(112, "task_completed", "task=2 envelope_task=Some(2)"))
        s.feed(observation(115))
        self.assertEqual(s.milestones["complete_view"], 15)
        self.assertFalse(s.done)
        s.feed(event(116, "task_completed", "task=1 envelope_task=Some(1)"))
        self.assertFalse(s.done)
        s.feed(observation(120))
        self.assertTrue(s.done)
        self.assertEqual(s.milestones["background_complete"], 20)

    def test_error_fallback_empty_library_cannot_be_success(self):
        s = bench.Sample()
        s.feed(event(5, "client_tls_ready"))
        s.feed(event(6, "request_queued", "seq=3 task=Some(1) msg=GetPlaylists depth=1"))
        s.feed(event(7, "response_error", 'seq=3 task=Some(1) error="failed"'))
        s.feed(observation(10))
        self.assertIsNotNone(s.error)

    def test_other_peers_failures_do_not_fail_our_task(self):
        s = bench.Sample()
        s.feed(event(1, "request_queued", "seq=1 task=Some(1) msg=Search depth=1"))
        s.feed(event(2, "task_failed", 'task=1 envelope_task=None error="peer failure"'))
        self.assertIsNone(s.error)
        s.feed(event(3, "task_failed", 'task=1 envelope_task=Some(1) error="our failure"'))
        self.assertIsNotNone(s.error)

    def test_reconnect_is_failed_instead_of_rebasing_the_clock(self):
        s = bench.Sample()
        s.feed(event(5, "client_tls_ready"))
        s.feed(event(6, "client_tls_ready"))
        self.assertIsNotNone(s.error)
        self.assertEqual(s.tls_ms, 5)

    def test_summary_retains_failures_and_uses_nearest_rank_p95(self):
        def good(value):
            return dict(status="ok", process_to_tls_ms=value, post_tls_ms={k: value for k in bench.MILESTONES})
        result = bench.summarize([good(i) for i in range(1, 21)] + [dict(status="timeout"), dict(status="failed")])
        self.assertEqual(result["samples"], 22)
        self.assertEqual(result["failed"], 1)
        self.assertEqual(result["timed_out"], 1)
        self.assertEqual(result["metrics"]["sidebar"], dict(n=20, median_ms=10.5, p95_ms=19))
        self.assertIsNone(bench.summarize([dict(status="failed")])["metrics"]["sidebar"]["p95_ms"])


class ProcessLifecycle(unittest.TestCase):
    def run_fake(self, body, timeout=2):
        import sys
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fake-mkp"
            binary.write_text(f"#!{sys.executable}\n" + body)
            binary.chmod(0o700)
            return bench.capture(binary, root, timeout, 40, 140, root / "trace.log")

    def test_real_pty_capture_drains_output_and_stops_after_success(self):
        trace = event(10, "client_tls_ready") + observation(20)
        result = self.run_fake(f"import os, time\nos.write(1, b'x' * 200000)\nos.write(2, {trace.encode()!r})\ntime.sleep(30)\n")
        self.assertEqual(result["status"], "ok", result)
        self.assertLess(result["wall_ms"], 2000)

    def test_hung_child_is_bounded(self):
        result = self.run_fake("import time\ntime.sleep(30)\n", timeout=0.15)
        self.assertEqual(result["status"], "timeout")
        self.assertLess(result["wall_ms"], 2000)

    def test_early_exit_is_not_success(self):
        result = self.run_fake("raise SystemExit(3)\n")
        self.assertEqual(result["status"], "failed")
        self.assertIn("exit 3", result["error"])


class CommandLine(unittest.TestCase):
    def test_repeated_runs_use_private_identical_config_and_report_warmups_separately(self):
        import subprocess
        import sys
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / "config"
            config.mkdir()
            (config / "last_server").write_text("original-server")
            binary = root / "fake-mkp"
            trace = event(10, "client_tls_ready") + observation(20)
            binary.write_text(f"#!{sys.executable}\n" +
                "import os, time\nfrom pathlib import Path\n" +
                "config = Path(os.environ['MKP_CONFIG_HOME'])\n" +
                "assert os.environ['RUST_LOG'] == 'mkp_startup=trace'\n" +
                "assert (config / 'last_server').read_text().strip() == 'selected'\n" +
                "assert not (config / 'written').exists()\n" +
                "(config / 'written').write_text('changed')\n" +
                f"os.write(2, {trace.encode()!r})\ntime.sleep(30)\n")
            binary.chmod(0o700)
            command = [sys.executable, str(Path(bench.__file__)), "--binary", str(binary),
                "--config", str(config), "--server", "selected", "--runs", "2", "--warmup", "1",
                "--fixture", "test", "--output", str(root / "report")]
            completed = subprocess.run(command, capture_output=True, text=True, timeout=10)
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            report = json.loads((root / "report/report.json").read_text())
            self.assertEqual(report["summary"]["samples"], 2)
            self.assertEqual(len(report["warmups"]), 1)
            self.assertEqual((config / "last_server").read_text(), "original-server")
            self.assertFalse((config / "written").exists())



if __name__ == "__main__":
    unittest.main()
