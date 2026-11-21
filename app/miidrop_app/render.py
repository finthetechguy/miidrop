"""Pictures of Miis from a Mii Renderer (REAL) compatible server, cached on disk."""

import hashlib
import os
import threading
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

DEFAULT_SERVER = "https://mii-unsecure.ariankordi.net"
TIMEOUT = 15


def _cache_dir():
    return Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "miidrop" / "pictures"


def image_url(server, data, width, view="face"):
    query = urllib.parse.urlencode(
        {"data": bytes(data).hex(), "type": view, "width": width, "bgColor": "00000000"}
    )
    return f"{server.rstrip('/')}/miis/image.png?{query}"


class Renderer:
    """Fetches PNGs off the main thread; `done` is called with the PNG bytes, or None on failure."""

    def __init__(self, server=DEFAULT_SERVER, cache=None, workers=2):
        self.server = server
        self.cache = Path(cache) if cache else _cache_dir()
        self._pool = ThreadPoolExecutor(max_workers=workers)
        self._lock = threading.Lock()
        self._waiting = {}

    def _path(self, url):
        return self.cache / (hashlib.sha256(url.encode()).hexdigest() + ".png")

    def cached(self, data, width, view="face"):
        try:
            return self._path(image_url(self.server, data, width, view)).read_bytes()
        except OSError:
            return None

    def request(self, data, width, done, view="face"):
        url = image_url(self.server, data, width, view)
        with self._lock:
            if url in self._waiting:
                self._waiting[url].append(done)
                return
            self._waiting[url] = [done]
        self._pool.submit(self._fetch, url)

    def _fetch(self, url):
        path = self._path(url)
        try:
            png = path.read_bytes()
        except OSError:
            png = self._download(url)
            if png:
                try:
                    self.cache.mkdir(parents=True, exist_ok=True)
                    tmp = path.with_suffix(".tmp")
                    tmp.write_bytes(png)
                    tmp.replace(path)
                except OSError:
                    pass
        with self._lock:
            callbacks = self._waiting.pop(url, [])
        for done in callbacks:
            done(png)

    @staticmethod
    def _download(url):
        req = urllib.request.Request(url, headers={"User-Agent": "miidrop"})
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
                body = r.read()
        except (urllib.error.URLError, OSError, ValueError):
            return None
        return body if body.startswith(b"\x89PNG") else None

    def close(self):
        self._pool.shutdown(wait=False, cancel_futures=True)
