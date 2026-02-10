"""codeix — Portable, composable code index."""

import os
import platform
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from io import BytesIO
from pathlib import Path
from urllib.request import urlopen, Request

__version__ = "0.3.0"  # x-release-please-version

REPO = "montanetech/codeix"

PLATFORM_MAP = {
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("Windows", "AMD64"): "x86_64-pc-windows-msvc",
}


def _cache_dir() -> Path:
    """Platform-appropriate cache directory."""
    if sys.platform == "win32":
        base = Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local"))
    elif sys.platform == "darwin":
        base = Path.home() / "Library" / "Caches"
    else:
        base = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))
    return base / "codeix" / "bin"


def _get_target() -> str:
    key = (platform.system(), platform.machine())
    target = PLATFORM_MAP.get(key)
    if not target:
        print(f"codeix: unsupported platform {key}", file=sys.stderr)
        print(f"Supported: {list(PLATFORM_MAP.keys())}", file=sys.stderr)
        sys.exit(1)
    return target


def _download(url: str) -> bytes:
    req = Request(url, headers={"User-Agent": f"codeix-python/{__version__}"})
    with urlopen(req) as resp:
        return resp.read()


def _ensure_binary() -> Path:
    """Download the binary if not cached, return its path."""
    target = _get_target()
    ext = ".exe" if sys.platform == "win32" else ""
    cache = _cache_dir()
    bin_path = cache / f"codeix-{__version__}{ext}"

    if bin_path.exists():
        return bin_path

    archive_ext = "zip" if sys.platform == "win32" else "tar.gz"
    url = f"https://github.com/{REPO}/releases/download/v{__version__}/codeix-{target}.{archive_ext}"

    print(f"Downloading codeix v{__version__} for {target}...", file=sys.stderr)
    data = _download(url)

    cache.mkdir(parents=True, exist_ok=True)

    if archive_ext == "zip":
        with zipfile.ZipFile(BytesIO(data)) as zf:
            for name in zf.namelist():
                if name.endswith(("codeix", "codeix.exe")):
                    bin_path.write_bytes(zf.read(name))
                    break
    else:
        with tarfile.open(fileobj=BytesIO(data), mode="r:gz") as tf:
            for member in tf.getmembers():
                if member.name.endswith("codeix"):
                    f = tf.extractfile(member)
                    if f:
                        bin_path.write_bytes(f.read())
                    break

    if not bin_path.exists():
        print("codeix: failed to extract binary from archive", file=sys.stderr)
        sys.exit(1)

    # Make executable on Unix
    if sys.platform != "win32":
        bin_path.chmod(bin_path.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)

    print(f"Cached codeix to {bin_path}", file=sys.stderr)
    return bin_path


def main() -> None:
    """Entry point: download binary if needed, then exec with forwarded args."""
    try:
        bin_path = _ensure_binary()
    except Exception as e:
        print(f"codeix: failed to download binary: {e}", file=sys.stderr)
        print(f"Install manually from: https://github.com/{REPO}/releases/tag/v{__version__}", file=sys.stderr)
        sys.exit(1)

    result = subprocess.run([str(bin_path)] + sys.argv[1:])
    sys.exit(result.returncode)
