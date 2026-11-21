"""Stands in for `miidrop serve`, answering commands with the events the real helper emits."""

import json
import sys


def emit(**event):
    print(json.dumps(event), flush=True)


emit(event="hello", version="0", mac="02:00:00:00:00:01", active_monitor=True)
emit(event="state", state="idle")
for line in sys.stdin:
    cmd = json.loads(line)
    kind = cmd["cmd"]
    if kind == "keys":
        emit(event="keys", beacon="udsBeaconKey=" in cmd["text"])
    elif kind == "join":
        emit(event="connected", role="client", peer="Console")
        emit(event="ready")
    elif kind == "send":
        emit(event="sending", name="Test")
        emit(event="verdict", accepted=len(cmd["record"]) == 192)
    elif kind == "quit":
        break
    else:
        emit(event="error", message="unexpected " + kind)
