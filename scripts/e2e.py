#!/usr/bin/env python3
"""End to end checks for Sesh on the booted simulator, against `just sshd`.

Run it with `just e2e`. It reinstalls the app, imports the test key, adds Hosts and
walks the phase 2 to 5 acceptance checks, printing one PASS or FAIL line per check.

Assertions read markers the remote shell writes under .local/e2e, because the sshd is
this Mac; screens that have no file to leave behind are read from the accessibility tree
or the pasteboard instead.

Traps this script works around:
  - `axe type` reaches the terminal only while it is the first responder; the keys row is
    an inputAccessoryView, so it is on screen whenever the keyboard is.
  - with the software keyboard up, `shift` matches two elements, so the keys row's own
    keys are tapped by coordinate inside the row, never by label.
  - keys scrolled out of the row's ScrollView still report their content coordinates, so
    a far key needs the row scrolled first.
  - `xcrun simctl pbcopy` fills the simulator pasteboard; a long key goes in that way and
    is pasted with cmd+V rather than typed.
  - the keys row moves down when Click or Select puts the keyboard away, so its y is
    measured again before every tap into it.
  - a form field's keyboard covers the rest of the form; every field is submitted with
    Return before the next control is tapped, and a Toggle only flips at the activation
    point axe resolves, not at the centre of its row.
"""

import json
import os
import shutil
import subprocess
import sys
import time

UDID = "466D3ACF-302A-40B8-8570-719088AF057C"
BUNDLE = "me.pecheny.sesh"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOCAL = os.path.join(ROOT, ".local")
MARKERS = os.path.join(LOCAL, "e2e")
SHOTS = os.path.join(LOCAL)
USER = os.environ.get("USER", "")

results = []


# ---------------------------------------------------------------- simulator plumbing


def run(*command, **kwargs):
    return subprocess.run(command, capture_output=True, text=True, **kwargs)


def axe(*args):
    result = run("axe", *args, "--udid", UDID)
    if result.returncode != 0:
        raise RuntimeError(f"axe {' '.join(args)}: {result.stdout}{result.stderr}")
    return result.stdout


def tree():
    flat = []

    def walk(node):
        for child in node.get("children") or []:
            frame = child.get("frame") or {}
            flat.append(
                {
                    "type": child.get("type"),
                    "label": child.get("AXLabel"),
                    "value": child.get("AXValue"),
                    "id": child.get("AXUniqueId"),
                    "x": frame.get("x", 0) + frame.get("width", 0) / 2,
                    "y": frame.get("y", 0) + frame.get("height", 0) / 2,
                    "frame": frame,
                }
            )
            walk(child)

    walk({"children": json.loads(axe("describe-ui"))})
    return flat


def match(label=None, kind=None, identifier=None, contains=None):
    found = []
    for element in tree():
        if label is not None and element["label"] != label:
            continue
        if contains is not None and contains not in (element["label"] or ""):
            continue
        if kind is not None and element["type"] != kind:
            continue
        if identifier is not None and element["id"] != identifier:
            continue
        found.append(element)
    return found


def wait_for(predicate, timeout=20, step=0.5):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(step)
    return None


def element(label=None, timeout=15, **kwargs):
    return wait_for(lambda: (match(label=label, **kwargs) or [None])[0], timeout)


def tap(x, y, settle=0.6):
    axe("tap", "-x", f"{x:.0f}", "-y", f"{y:.0f}")
    time.sleep(settle)


def tap_activation(label, settle=0.8):
    """Let axe resolve the activation point: a Toggle's row centre does not toggle it."""
    axe("tap", "--label", label, "--wait-timeout", "10")
    time.sleep(settle)


def tap_label(label, timeout=15, settle=0.8, **kwargs):
    found = element(label, timeout=timeout, **kwargs)
    if not found:
        raise RuntimeError(f"no element labelled {label!r}")
    tap(found["x"], found["y"], settle)


def type_text(text):
    subprocess.run(["axe", "type", "--stdin", "--udid", UDID], input=text, text=True, check=True)
    time.sleep(0.3)


def press(keycode):
    axe("key", str(keycode))
    time.sleep(0.3)


def combo(modifier, keycode):
    axe("key-combo", "--modifiers", str(modifier), "--key", str(keycode))
    time.sleep(0.4)


def shot(name):
    axe("screenshot", "--output", os.path.join(SHOTS, f"phase5-{name}.png"))


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    print(f"{'PASS' if ok else 'FAIL'} {name}{'  ' + detail if detail and not ok else ''}", flush=True)


# ------------------------------------------------------------------- the remote shell

RETURN, CMD, A_KEY, V_KEY = 40, 227, 4, 25


def marker(name):
    return os.path.join(MARKERS, name)


def shell(command, settle=0.6):
    type_text(command)
    press(RETURN)
    time.sleep(settle)


def wait_marker(name, timeout=15):
    path = marker(name)
    if wait_for(lambda: os.path.exists(path) and open(path).read(), timeout):
        return open(path).read()
    return ""


def terminal():
    found = element(identifier="terminal", timeout=20)
    if not found:
        raise RuntimeError("the terminal view never appeared")
    return found["frame"]


# ------------------------------------------------------------------------- the screens


def install():
    run("xcrun", "simctl", "terminate", UDID, BUNDLE)
    run("xcrun", "simctl", "uninstall", UDID, BUNDLE)
    app = os.path.join(ROOT, "build/Build/Products/Debug-iphonesimulator/Sesh.app")
    if run("xcrun", "simctl", "install", UDID, app).returncode != 0:
        sys.exit("install failed: run `just build` first")
    run("xcrun", "simctl", "launch", UDID, BUNDLE)
    time.sleep(3)


def import_key():
    tap_label("Keys")
    tap_label("Import")
    editor = element(kind="TextArea", timeout=10) or {"x": 200, "y": 450}
    tap(editor["x"], editor["y"])
    with open(os.path.join(LOCAL, "testkey")) as handle:
        subprocess.run(["xcrun", "simctl", "pbcopy", UDID], input=handle.read(), text=True)
    time.sleep(0.5)
    combo(CMD, V_KEY)
    time.sleep(1)
    confirm = [e for e in match(label="Import", kind="Button") if e["y"] < 130]
    tap(confirm[-1]["x"], confirm[-1]["y"], settle=1.5)
    ok = bool(match(contains="ssh-ed25519"))
    check("the test Key imports", ok)
    tap_label("Done")


def fill(label, text):
    found = element(label, kind="TextField")
    tap(found["x"], found["y"], settle=0.4)
    combo(CMD, A_KEY)
    type_text(text)
    press(RETURN)  # the software keyboard would otherwise cover the rest of the form


def add_host(name, transport, agent):
    tap_label("Add Host")
    fill("Name", name)
    fill("Address", "localhost")
    fill("Port", "2222")
    fill("User", USER)
    if transport != "ssh":
        tap_label("Transport, SSH")
        tap_label("mosh")
    tap_label("Key, None")
    tap_label("ssh-ed25519")
    if agent:
        tap_activation("Agent forwarding")
        on = element("Agent forwarding", kind="CheckBox")
        check("the agent toggle turns on", on and on["value"] == "1", str(on))
    tap_label("Save", settle=1.2)
    check(f"the {name} Host is saved", bool(match(label=name)))


def open_host(name, first_time):
    tap_label(name, settle=1.5)
    if first_time:
        asked = element("Accept", timeout=20)
        check("the host key is offered once", bool(asked))
        if asked:
            tap(asked["x"], asked["y"], settle=1.5)
    else:
        check(f"{name} reconnects without asking again", not match(label="Accept"))
    connected = wait_for(lambda: match(label="connected"), timeout=25)
    check(f"the {name} Session connects", bool(connected))


# --------------------------------------------------------------------------- the checks


def check_command():
    shell(f"ls /etc/hosts > {marker('ls.txt')} 2>&1")
    check("a command runs over ssh", "/etc/hosts" in wait_marker("ls.txt"))


def check_agent(expected, name):
    shell(f"ssh-add -l > {marker('agent.txt')} 2>&1")
    seen = wait_marker("agent.txt", timeout=10)
    listed = "ssh-ed25519" in seen or "ED25519" in seen
    check(name, listed == expected, seen.strip())


def check_title():
    shell("printf '\\033]0;SESHTITLE\\007'")
    check("the remote title reaches the Tab bar", bool(wait_for(lambda: match(label="SESHTITLE"), 8)))


def keys_row():
    """The row's own frame, so its keys are tapped by coordinate and never by label."""
    esc = element("esc", timeout=10)
    if not esc:
        raise RuntimeError("the keys row is not on screen")
    return esc["frame"]["y"] + esc["frame"]["height"] / 2


def tap_key(label, row_y):
    found = [e for e in match(label=label) if abs(e["y"] - row_y) < 24]
    if not found:
        raise RuntimeError(f"no keys-row key {label!r}")
    tap(found[0]["x"], row_y, settle=0.4)


def check_modes():
    keys = keys_row()
    mode("Click")
    check("Click mode puts the keyboard down", not match(label="q") and bool(match(label="esc")))
    selector = element("mode", timeout=5)
    check("the selector carries the mode", selector and selector["value"] == "Click", str(selector))
    shell(f"echo CLICKMODE > {marker('click.txt')}")
    check("a hardware keyboard types in Click mode", "CLICKMODE" in wait_marker("click.txt", timeout=8))
    mode("Type")
    check("Type mode brings the keyboard back", bool(match(label="q")) and abs(keys_row() - keys) < 1)


def check_interrupt():
    row = keys_row()
    shell("sleep 100", settle=1.5)
    tap_key("ctrl", row)
    type_text("c")
    time.sleep(1)
    shell(f"echo INTERRUPTED > {marker('sigint.txt')}")
    check("ctrl-c interrupts sleep 100", "INTERRUPTED" in wait_marker("sigint.txt", timeout=8))


def mode(name):
    tap_key(name, keys_row())


def check_selection():
    shell("clear; echo WORDMARKER", settle=1.2)
    subprocess.run(["xcrun", "simctl", "pbcopy", UDID], input="", text=True)
    mode("Select")
    frame = terminal()
    line = frame["y"] + 8
    axe("drag", "--start-x", f"{frame['x'] + 2:.0f}", "--start-y", f"{line:.0f}",
        "--end-x", f"{frame['x'] + frame['width'] - 4:.0f}", "--end-y", f"{line:.0f}",
        "--duration", "1.0")
    time.sleep(1)
    copied = ""
    if element("Copy", timeout=6):
        tap_label("Copy", settle=1.0)
        copied = run("xcrun", "simctl", "pbpaste", UDID).stdout
    check("select mode copies a word", "WORDMARKER" in copied, copied.strip())
    mode("Type")


def check_draft():
    row = keys_row()
    shell(f"cat > {marker('draft.txt')}", settle=1.0)
    tap_key("editor", row)
    tap_label("New Draft")
    body = element(kind="TextArea", timeout=10)
    tap(body["x"], body["y"])
    type_text("draft line one")
    press(RETURN)
    type_text("draft line two")
    tap_label("Send + Enter", settle=1.5)
    row = keys_row()
    tap_key("ctrl", row)
    type_text("d")
    time.sleep(1)
    text = wait_marker("draft.txt", timeout=8)
    check("a two-line Draft arrives whole", "draft line one\ndraft line two" in text, repr(text))


def check_reconnect():
    shell("exit", settle=2.0)
    overlay = element("Reconnect", timeout=15)
    check("a finished Session shows the overlay", bool(overlay))
    shot("overlay")
    if overlay:
        tap(overlay["x"], overlay["y"], settle=2.0)
    connected = wait_for(lambda: match(label="connected"), timeout=25)
    check("Reconnect starts a new Session in the Tab", bool(connected))
    check("the host key is not asked again", not match(label="Accept"))
    shell(f"echo RECONNECTED > {marker('reconnect.txt')}")
    check("the reconnected Session runs a command", "RECONNECTED" in wait_marker("reconnect.txt", timeout=10))


def check_second_tab(name):
    tap_label("Tabs")
    shot("switcher")
    tap_label("Open another Host", settle=1.2)
    check("the switcher can reach the Host list", bool(match(label="Add Host")))
    check("the Host list offers the way back to the Tabs", bool(match(label="Tabs")))
    shot("hosts")
    open_host(name, first_time=False)
    count = element("Tabs", timeout=10)
    check("a second Tab is open", count and count["value"] == "2", str(count))
    shot("tabs")


def show_tab(name):
    tap_label("Tabs")
    row = element(contains=f"{name},", timeout=10)
    tap(row["x"], row["y"], settle=1.5)


def copy_screen():
    subprocess.run(["xcrun", "simctl", "pbcopy", UDID], input="", text=True)
    frame = terminal()
    mode("Select")
    axe("drag", "--start-x", f"{frame['x'] + 2:.0f}", "--start-y", f"{frame['y'] + 8:.0f}",
        "--end-x", f"{frame['x'] + frame['width'] - 4:.0f}",
        "--end-y", f"{frame['y'] + frame['height'] - 24:.0f}", "--duration", "1.2")
    time.sleep(1)
    text = ""
    if element("Copy", timeout=6):
        tap_label("Copy", settle=1.0)
        text = run("xcrun", "simctl", "pbpaste", UDID).stdout
    mode("Type")
    return text


def check_hidden_tab(here, there):
    shell("(sleep 8; echo HIDDENOUT) &", settle=1.0)
    show_tab(there)
    time.sleep(12)
    show_tab(here)
    check("a hidden Tab keeps reading its Session", "HIDDENOUT" in copy_screen())


def check_close_all():
    for _ in range(6):
        if not match(label="Close"):
            break
        tap_label("Close", settle=1.2)
    check("closing the last Tab shows the Host list", bool(match(label="Add Host")))


def check_mosh(name):
    tap_label("Tabs")
    tap_label("Open another Host", settle=1.2)
    open_host(name, first_time=False)
    shell(f"echo MOSH-BEFORE > {marker('mosh1.txt')}")
    check("a mosh Session runs a command", "MOSH-BEFORE" in wait_marker("mosh1.txt", timeout=15))
    shot("mosh")
    axe("button", "home")
    time.sleep(30)
    run("xcrun", "simctl", "launch", UDID, BUNDLE)
    time.sleep(4)
    shell(f"echo MOSH-AFTER > {marker('mosh2.txt')}", settle=1.0)
    check("the mosh Session survives backgrounding", "MOSH-AFTER" in wait_marker("mosh2.txt", timeout=20))


def check_density():
    row = keys_row()
    wanted = ["esc", "ctrl", "alt", "shift", "cmd", "rclk"]
    visible = []
    for label in wanted:
        found = [e for e in match(label="right-click" if label == "rclk" else label)
                 if abs(e["y"] - row) < 24]
        if found and found[0]["frame"]["x"] + found[0]["frame"]["width"] <= 402:
            visible.append(label)
    check("the keys row shows esc to right-click without scrolling", visible == wanted, str(visible))


# ------------------------------------------------------------------------------- main


def main():
    if run("bash", os.path.join(ROOT, "scripts/local-sshd.sh")).returncode != 0:
        sys.exit("the local sshd would not start")
    shutil.rmtree(MARKERS, ignore_errors=True)
    os.makedirs(MARKERS, exist_ok=True)

    install()
    import_key()
    add_host("agent", "ssh", agent=True)
    add_host("plain", "ssh", agent=False)
    add_host("moshy", "mosh", agent=False)

    open_host("agent", first_time=True)
    shot("connected")
    check_command()
    check_agent(True, "agent forwarding lists the Key on the remote")
    check_title()
    check_density()
    check_modes()
    check_interrupt()
    check_selection()
    check_draft()
    check_reconnect()

    check_second_tab("plain")
    check_agent(False, "a Host without the toggle has no agent")
    check_hidden_tab("plain", "SESHTITLE")
    check_mosh("moshy")
    shot("final")
    check_close_all()

    failed = [name for name, ok in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} checks passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
