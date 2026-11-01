"""Hands panes over from herdr-sesh to sesh-mux and checks they survive.

Builds herdr-sesh from a copy of HERDR_SRC (default ~/herdr) and sesh-mux from this checkout,
starts herdr-sesh under a temporary HOME, opens a pane running `cat` over some scrollback and a
pane running a stand-in agent, hands off to sesh-mux, and checks the panes, their processes,
their scrollback, the legacy flag and the old socket. Then it replaces the stand-in agent and
hands sesh-mux off to itself. Run with `uv run python mux/scripts/migration_test.py`.
"""

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

MUX = Path(__file__).resolve().parent.parent
HERDR_SRC = Path(os.environ.get("HERDR_SRC", Path.home() / "herdr"))
STAND_IN_AGENT = "bash -c 'exec -a claude sleep 100000'"


def build(tmp: Path) -> tuple[Path, Path]:
    src = tmp / "herdr-src"
    subprocess.run(
        ["rsync", "-a", "--exclude", "target", "--exclude", ".zig-cache", "--exclude", "zig-out",
         "--exclude", ".git", f"{HERDR_SRC}/", f"{src}/"],
        check=True,
    )
    target = Path(os.environ.get("MIGRATION_TEST_TARGET", tmp / "target"))
    subprocess.run(["cargo", "build", "--release", "--quiet"], cwd=src, check=True,
                   env={**os.environ, "CARGO_TARGET_DIR": str(target)})
    subprocess.run(["cargo", "build", "--release", "--quiet"], cwd=MUX, check=True)
    return target / "release" / "herdr", MUX / "target" / "release" / "sesh-mux"


class Host:
    def __init__(self, tmp: Path, herdr: Path, mux: Path):
        self.home = tmp / "home"
        self.home.mkdir()
        self.herdr, self.mux = herdr, mux
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("HERDR_", "SESH_MUX_", "XDG_"))}
        self.env.update(HOME=str(self.home), SHELL="/bin/bash", TERM="xterm-256color")
        self.old_socket = self.home / ".config/herdr/sessions/sesh/herdr.sock"
        self.wrapper = tmp / "sesh-mux-import"
        self.wrapper.write_text(f'#!/bin/sh\nexec "{mux}" "$@"\n')
        self.wrapper.chmod(0o755)

    def run(self, binary: Path, *args: str, check: bool = True) -> dict:
        command = [str(binary)] + (["--session", "sesh"] if binary == self.herdr else []) + list(args)
        done = subprocess.run(command, env=self.env, capture_output=True, text=True, timeout=60)
        if check and done.returncode != 0:
            raise AssertionError(f"{' '.join(args)} failed: {done.stderr or done.stdout}")
        return json.loads(done.stdout) if done.stdout.strip().startswith("{") else {"text": done.stdout}

    def herdr_cli(self, *args: str) -> dict:
        return self.run(self.herdr, *args)

    def mux_cli(self, *args: str, check: bool = True) -> dict:
        return self.run(self.mux, *args, check=check)

    def wait(self, what: str, predicate, timeout: float = 15.0):
        deadline = time.time() + timeout
        last = None
        while time.time() < deadline:
            try:
                value = predicate()
                if value:
                    return value
            except (AssertionError, subprocess.TimeoutExpired, json.JSONDecodeError) as err:
                last = err
            time.sleep(0.2)
        raise AssertionError(f"timed out waiting for {what}: {last}")


def foreground_pid(host: Host, cli, pane: str, name: str) -> int:
    info = cli("pane", "process-info", "--pane", pane)["result"]["process_info"]
    for process in info.get("foreground_processes", []):
        if name in (process.get("argv0") or process["name"]):
            return process["pid"]
    raise AssertionError(f"{name} is not in the foreground of {pane}: {info}")


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def pane_json(host: Host, pane: str) -> dict:
    return host.mux_cli("pane", "get", pane)["result"]["pane"]


def main() -> int:
    # Unix socket paths must stay short, so the temporary HOME lives under /tmp.
    tmp = Path(tempfile.mkdtemp(prefix="mm-", dir="/tmp"))
    server_pids: list[int] = []
    host = None
    try:
        herdr, mux = build(tmp)
        host = Host(tmp, herdr, mux)
        log = open(tmp / "herdr-server.log", "w")
        server = subprocess.Popen([str(herdr), "--session", "sesh", "server"], env=host.env,
                                  stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                  start_new_session=True)
        server_pids.append(server.pid)
        host.wait("herdr-sesh", lambda: host.herdr_cli("workspace", "list"))

        created = host.herdr_cli("workspace", "create", "--cwd", str(tmp), "--label", "cat", "--no-focus")
        workspace = created["result"]["workspace"]["workspace_id"]
        cat_pane = created["result"]["root_pane"]["pane_id"]
        host.herdr_cli("pane", "run", cat_pane, "for i in $(seq 1 60); do echo scrollback-$i; done; cat")
        cat_pid = host.wait("cat", lambda: foreground_pid(host, host.herdr_cli, cat_pane, "cat"))
        server_pids.append(cat_pid)
        agent_pane = host.herdr_cli("tab", "create", "--workspace", workspace, "--cwd", str(tmp),
                                    "--label", "agent", "--no-focus")["result"]["root_pane"]["pane_id"]
        host.herdr_cli("pane", "run", agent_pane, STAND_IN_AGENT)
        agent_pid = host.wait("stand-in agent",
                              lambda: foreground_pid(host, host.herdr_cli, agent_pane, "claude"))
        server_pids.append(agent_pid)
        print(f"herdr-sesh: cat {cat_pane} pid {cat_pid}, agent {agent_pane} pid {agent_pid}")

        host.herdr_cli("server", "live-handoff", "--import-exe", str(host.wrapper))
        host.wait("sesh-mux after handoff", lambda: host.mux_cli("workspace", "list"))
        panes = {pane["pane_id"]: pane for pane in host.mux_cli("pane", "list")["result"]["panes"]}
        assert cat_pane in panes and agent_pane in panes, f"panes after handoff: {sorted(panes)}"
        assert alive(cat_pid) and alive(agent_pid), "a pane process died in the handoff"
        recent = host.mux_cli("pane", "read", cat_pane, "--source", "recent", "--lines", "100")
        assert "scrollback-1\n" in recent["text"] and "scrollback-60" in recent["text"], recent
        host.mux_cli("pane", "run", cat_pane, "echo-after-handoff")
        host.wait("cat to echo", lambda: host.mux_cli(
            "pane", "read", cat_pane, "--source", "recent", "--lines", "20")["text"].count("echo-after-handoff") >= 2)
        print("sesh-mux: both panes, their processes and the scrollback survived")

        assert host.wait("legacy flag", lambda: pane_json(host, agent_pane).get("legacy") is True)
        assert "legacy" not in pane_json(host, cat_pane), "a pane without an agent is legacy"
        assert host.old_socket.exists(), "the old socket is not served while a legacy agent runs"
        report = subprocess.run(
            ["nc", "-U", str(host.old_socket)], capture_output=True, text=True, timeout=10,
            input=json.dumps({"id": "legacy-hook", "method": "pane.report_agent", "params": {
                "pane_id": agent_pane, "source": "herdr:claude", "agent": "claude", "state": "working"}}) + "\n")
        assert '"ok"' in report.stdout, f"legacy socket report: {report.stdout or report.stderr}"
        print("sesh-mux: the agent pane is legacy and the old socket accepts herdr:claude")

        os.kill(agent_pid, signal.SIGTERM)
        host.wait("the stand-in agent to exit", lambda: not alive(agent_pid))
        host.mux_cli("pane", "run", agent_pane, STAND_IN_AGENT)
        new_agent = host.wait("a new stand-in agent",
                              lambda: foreground_pid(host, host.mux_cli, agent_pane, "claude"))
        server_pids.append(new_agent)
        assert new_agent != agent_pid
        host.wait("legacy to clear", lambda: "legacy" not in pane_json(host, agent_pane))
        host.wait("the old socket to close", lambda: not host.old_socket.exists())
        print(f"sesh-mux: after replacing the agent (pid {new_agent}) the pane is not legacy and the old socket is closed")

        host.mux_cli("server", "live-handoff")
        host.wait("sesh-mux after its own handoff", lambda: host.mux_cli("pane", "get", cat_pane))
        assert alive(cat_pid) and alive(new_agent), "a pane process died in the sesh-mux handoff"
        host.mux_cli("pane", "run", cat_pane, "echo-after-second-handoff")
        host.wait("cat to echo again", lambda: host.mux_cli(
            "pane", "read", cat_pane, "--source", "recent", "--lines", "20")["text"].count("echo-after-second-handoff") >= 2)
        print("sesh-mux -> sesh-mux: panes and processes survived")
        host.mux_cli("server", "stop", check=False)
        print("PASS")
        return 0
    finally:
        if host is not None:
            host.mux_cli("server", "stop", check=False)
            for binary in (host.herdr,):
                subprocess.run([str(binary), "--session", "sesh", "server", "stop"], env=host.env,
                               capture_output=True, timeout=30)
        for pid in server_pids:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        if os.environ.get("MIGRATION_TEST_KEEP"):
            print(f"kept {tmp}")
        else:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
