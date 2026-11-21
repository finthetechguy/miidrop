"""Client for the privileged radio helper, `miidrop serve`, which speaks JSON lines on stdio."""

import json
import os
import shlex
import shutil
import subprocess
import threading
from collections import deque


def helper_command(interface):
    """Returns the argv that starts the helper, going through pkexec unless already root."""
    override = os.environ.get("MIIDROP_HELPER")
    if override:
        return shlex.split(override) + ["serve", "-i", interface]
    binary = os.environ.get("MIIDROP_BIN") or shutil.which("miidrop")
    if not binary:
        raise FileNotFoundError("'miidrop' is not installed")
    binary = os.path.abspath(binary)
    argv = [binary, "serve", "-i", interface]
    return argv if os.geteuid() == 0 else ["pkexec"] + argv


class Helper:
    """Runs the helper and passes each event dict to `on_event` from a background thread, ending with an "exit" event."""

    def __init__(self, on_event):
        self._on_event = on_event
        self._proc = None
        self._stderr = deque(maxlen=20)
        self._lock = threading.Lock()

    @property
    def running(self):
        return self._proc is not None and self._proc.poll() is None

    def start(self, interface, keys_path=None):
        if self.running:
            raise RuntimeError("helper already running")
        self._proc = subprocess.Popen(
            helper_command(interface),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        threading.Thread(target=self._read_stderr, args=(self._proc,), daemon=True).start()
        threading.Thread(target=self._read_stdout, args=(self._proc,), daemon=True).start()
        if keys_path:
            self.load_keys(keys_path)

    def _read_stdout(self, proc):
        for line in proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if isinstance(event, dict) and "event" in event:
                self._on_event(event)
        code = proc.wait()
        self._on_event({"event": "exit", "code": code, "stderr": "\n".join(self._stderr)})

    def _read_stderr(self, proc):
        for line in proc.stderr:
            self._stderr.append(line.rstrip())

    def _send(self, **command):
        with self._lock:
            if not self.running:
                raise RuntimeError("helper is not running")
            self._proc.stdin.write(json.dumps(command) + "\n")
            self._proc.stdin.flush()

    def load_keys(self, path):
        # The key file is read here, as the user, so the helper never opens user paths.
        with open(path, encoding="utf-8") as f:
            self.send_keys(f.read())

    def send_keys(self, text):
        self._send(cmd="keys", text=text)

    def scan(self):
        self._send(cmd="scan")

    def join(self, host=None, name="miidrop"):
        self._send(cmd="join", host=host, name=name)

    def host(self, channel=6, name="miidrop"):
        self._send(cmd="host", channel=channel, name=name)

    def send_mii(self, record):
        """Sends a 92- or 96-byte record once both sides are ready."""
        self._send(cmd="send", record=bytes(record).hex())

    def stop(self):
        self._send(cmd="stop")

    def close(self, timeout=5):
        if not self.running:
            return
        try:
            self._send(cmd="quit")
            self._proc.stdin.close()
            self._proc.wait(timeout)
        except (RuntimeError, OSError, subprocess.TimeoutExpired):
            self._proc.terminate()
