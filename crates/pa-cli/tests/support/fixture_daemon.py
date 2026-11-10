#!/usr/bin/env python3
"""A fixture daemon for the installer's stop probe: binds a socket, greets every
connection the way a real supervisor does, then answers the probe's envelope.

argv: <socket> <pid|self> <deaf|honour>
env:  FIXTURE_EXECUTABLE   - the path the hello advertises as runtime.executablePath
      FIXTURE_START_ID     - the process start id the hello CLAIMS (a forged value)
      FIXTURE_OMIT_START_ID - "1" leaves supervisorProcessStartId out of the hello
"""
import json, os, socket, subprocess, sys

def kernel_start_id(pid):
    # The SAME identity the probe reads (`proc:<field 22>`, else `ps:<lstart>` under
    # the pinned locale): the shipped Rust supervisor reports exactly this, so a
    # fixture that advertises anything else is the anomaly under test.
    try:
        with open("/proc/%d/stat" % pid, "rb") as handle:
            stat = handle.read().decode("utf-8", "replace")
        return "proc:" + stat.rsplit(")", 1)[-1].split()[19]
    except (OSError, IndexError):
        pass
    try:
        out = subprocess.check_output(
            ["ps", "-p", str(pid), "-o", "lstart="],
            stderr=subprocess.DEVNULL, universal_newlines=True, timeout=5,
            env=dict(os.environ, LC_ALL="C", LC_TIME="C", LANG="C", TZ="UTC")).strip()
    except Exception:
        return None
    return "ps:" + out if out else None

path, pid_arg, mode = sys.argv[1], sys.argv[2], sys.argv[3]
if os.path.exists(path):
    os.unlink(path)
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
server.listen(8)
print(os.getpid(), flush=True)
pid = os.getpid() if pid_arg == "self" else int(pid_arg)
hello = {
    "type": "daemon_hello",
    "socketPath": path,
    "protocol": {"name": "prime-agent.daemon", "version": 7},
    "schemaId": "protocol-7-schema-30-8e4b17c2a9f5",
    "appVersion": "0.0.0",
    "runtime": {"buildId": "pa-daemon-rs-0.0.0",
                "executablePath": os.environ.get("FIXTURE_EXECUTABLE", "")},
    "supervisorPid": pid,
}
# The pid-reuse identity, the way the shipped supervisor carries it: the claimed
# pid's OWN start id by default, a forged one under FIXTURE_START_ID, and absent
# under FIXTURE_OMIT_START_ID (an older build's hello).
if os.environ.get("FIXTURE_OMIT_START_ID") != "1":
    claimed = os.environ.get("FIXTURE_START_ID") or kernel_start_id(pid)
    if claimed:
        hello["supervisorProcessStartId"] = claimed
while True:
    conn, _ = server.accept()
    try:
        with conn:
            # The greeting comes FIRST, exactly like the supervisor's: a client
            # (and the probe) reads it before it sends anything.
            conn.sendall((json.dumps(hello) + "\n").encode())
            line = b""
            while b"\n" not in line:
                part = conn.recv(4096)
                if not part:
                    break
                line += part
            if not line:
                continue
            request = json.loads(line.decode())
            kind = (request.get("command") or {}).get("type", "")
            # An EMPTY session list: the probe scales its drain window by the
            # count (2s per session), and this fixture has none, so the
            # regression test is not paying for a drain it does not exercise.
            data = {"sessions": []} if kind == "list" else {}
            reply = {"type": "response", "id": request.get("id"), "success": True,
                     "data": data}
            conn.sendall((json.dumps(reply) + "\n").encode())
            if kind == "shutdown" and mode == "honour":
                os.unlink(path)
                sys.exit(0)
    except (BrokenPipeError, ConnectionResetError):
        continue
