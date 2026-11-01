#!/bin/sh
# installed by sesh-mux
# managed by sesh-mux; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# SESH_MUX_INTEGRATION_ID=claude
# SESH_MUX_INTEGRATION_VERSION=11

set -eu

action="${1:-}"
hook_input_file="$(mktemp "${TMPDIR:-/tmp}/sesh-mux-claude-hook.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

case "$action" in
  session|permission) ;;
  *) exit 0 ;;
esac

[ "${SESH_MUX_ENV:-}" = "1" ] || exit 0
[ -n "${SESH_MUX_SOCKET_PATH:-}" ] || exit 0
[ -n "${SESH_MUX_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

SESH_MUX_ACTION="$action" SESH_MUX_HOOK_INPUT_FILE="$hook_input_file" python3 - <<'PY'
import json
import os
import random
import socket
import time

source = "sesh-mux:claude"
action = os.environ.get("SESH_MUX_ACTION", "")
pane_id = os.environ.get("SESH_MUX_PANE_ID")
socket_path = os.environ.get("SESH_MUX_SOCKET_PATH")
hook_input_file = os.environ.get("SESH_MUX_HOOK_INPUT_FILE")

if not pane_id or not socket_path:
    raise SystemExit(0)

hook_input = {}
if hook_input_file:
    try:
        with open(hook_input_file, encoding="utf-8") as handle:
            content = handle.read()
        if content.strip():
            hook_input = json.loads(content)
    except Exception:
        hook_input = {}

if "CURSOR_VERSION" in os.environ or "cursor_version" in hook_input:
    raise SystemExit(0)
hook_event_name = str(hook_input.get("hook_event_name") or "")
if hook_event_name != ("PermissionRequest" if action == "permission" else "SessionStart"):
    raise SystemExit(0)
is_subagent = bool(hook_input.get("agent_id"))
if is_subagent:
    raise SystemExit(0)
request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
report_seq = time.time_ns()
session_id = hook_input.get("session_id")
agent_session_id = session_id if isinstance(session_id, str) and session_id else None
transcript_path = hook_input.get("transcript_path")
agent_session_path = transcript_path if isinstance(transcript_path, str) and transcript_path else None
session_start_source = hook_input.get("source") if hook_event_name == "SessionStart" else None
if not isinstance(session_start_source, str) or not session_start_source:
    session_start_source = None
if action == "permission":
    tool_name = hook_input.get("tool_name")
    if not isinstance(tool_name, str) or not tool_name:
        raise SystemExit(0)
    request = {
        "id": request_id,
        "method": "pane.report_permission",
        "params": {
            "pane_id": pane_id,
            "source": source,
            "agent": "claude",
            "tool": tool_name,
            "input": hook_input.get("tool_input"),
        },
    }
elif agent_session_id:
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": "claude",
        "seq": report_seq,
        "agent_session_id": agent_session_id,
    }
    if agent_session_path:
        params["agent_session_path"] = agent_session_path
    if session_start_source:
        params["session_start_source"] = session_start_source
    request = {
        "id": request_id,
        "method": "pane.report_agent_session",
        "params": params,
    }
else:
    raise SystemExit(0)

try:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(0.5)
    client.connect(socket_path)
    client.sendall((json.dumps(request) + "\n").encode())
    try:
        client.recv(4096)
    except Exception:
        pass
    client.close()
except Exception:
    pass
PY
