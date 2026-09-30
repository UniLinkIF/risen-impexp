"""The hand-over to risen-core.exe: one subprocess per call, one JSON value back on stdout."""

import json
import os
import subprocess
import tempfile

from .prefs import prefs

_HERE = os.path.dirname(os.path.realpath(__file__))
# A packaged add-on ships the exe in bin/; a development checkout finds the cargo build next door.
_CANDIDATES = (
    os.path.join(_HERE, "bin", "risen-core.exe"),
    os.path.normpath(os.path.join(_HERE, "..", "..", "core", "target", "release", "risen-core.exe")),
)


class CoreError(RuntimeError):
    pass


def exe():
    p = prefs().core_exe
    if p:
        if not os.path.isfile(p):
            raise CoreError(f"risen-core.exe не знайдено: {p}")
        return p
    for c in _CANDIDATES:
        if os.path.isfile(c):
            return c
    raise CoreError("risen-core.exe не знайдено — вкажіть його в налаштуваннях аддона")


def cache_dir():
    d = prefs().cache_dir or os.path.join(tempfile.gettempdir(), "risen_impexp")
    os.makedirs(d, exist_ok=True)
    return d


def game_dir():
    d = prefs().game_dir
    if not d or not os.path.isfile(os.path.join(d, "bin", "Risen.exe")):
        raise CoreError(f"У папці гри немає bin\\Risen.exe: {d!r} — вкажіть її в налаштуваннях аддона")
    return d


def run(*args):
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    r = subprocess.run([exe(), *args], capture_output=True, text=True, encoding="utf-8", creationflags=flags)
    if r.returncode != 0:
        raise CoreError(r.stderr.strip() or f"risen-core завершився з кодом {r.returncode}")
    return json.loads(r.stdout)


_names = None


def mesh_names():
    """Every mesh name in the game, read once per Blender session (≈1 s)."""
    global _names
    if _names is None:
        _names = [f["name"] for f in run("find", game_dir(), "", "1000000")]
    return _names
