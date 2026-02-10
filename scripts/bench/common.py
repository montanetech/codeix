"""Common utilities for benchmark suite."""

import hashlib
import json
import os
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

# Project root (codeix repo)
PROJECT_ROOT = Path(__file__).parent.parent.parent

# Configuration
# Cache dir: persistent across runs (only for response cache)
CACHE_DIR = Path(os.environ.get("CODEIX_BENCH_CACHE", Path(tempfile.gettempdir()) / "codeix-bench-cache"))
RESPONSE_CACHE_DIR = CACHE_DIR / "responses"


@dataclass
class RunContext:
    """Context for a benchmark run - fresh temp directory per run."""
    run_dir: Path      # Root: /tmp/codeix-bench-xxxxx/
    bin_dir: Path      # Binaries: run_dir/bin/
    repos: Path        # Repos: run_dir/repos/ (for single-variant benchmarks)
    repos_a: Path      # Repos for A: run_dir/repos/a/ (for A/B benchmarks)
    repos_b: Path      # Repos for B: run_dir/repos/b/ (for A/B benchmarks)
    results_dir: Path  # Results: run_dir/results/


def create_run_context() -> RunContext:
    """Create a fresh temporary run directory with standard structure."""
    run_dir = Path(tempfile.mkdtemp(prefix="codeix-bench-"))
    ctx = RunContext(
        run_dir=run_dir,
        bin_dir=run_dir / "bin",
        repos=run_dir / "repos",
        repos_a=run_dir / "repos" / "a",
        repos_b=run_dir / "repos" / "b",
        results_dir=run_dir / "results",
    )
    ctx.bin_dir.mkdir(parents=True)
    ctx.repos.mkdir(parents=True)
    ctx.repos_a.mkdir(parents=True)
    ctx.repos_b.mkdir(parents=True)
    ctx.results_dir.mkdir(parents=True)
    return ctx


@dataclass
class Repo:
    name: str
    url: str
    lang: str
    size: str
    notes: str


REPOS = [
    # Small repos
    Repo("zod", "https://github.com/colinhacks/zod", "TypeScript", "small", "Validation library"),
    Repo("gin", "https://github.com/gin-gonic/gin", "Go", "small", "HTTP framework"),
    Repo("leveldb", "https://github.com/google/leveldb", "C++", "small", "KV store, has submodules"),
    # Medium repos
    Repo("tokio", "https://github.com/tokio-rs/tokio", "Rust", "medium", "Async runtime"),
    Repo("flask", "https://github.com/pallets/flask", "Python", "medium", "Web micro-framework"),
    Repo("junit5", "https://github.com/junit-team/junit5", "Java", "medium", "Testing framework"),
    Repo("libsodium", "https://github.com/jedisct1/libsodium", "C", "medium", "Crypto library"),
    Repo("faker", "https://github.com/faker-ruby/faker", "Ruby", "medium", "Data generation"),
    Repo("Newtonsoft.Json", "https://github.com/JamesNK/Newtonsoft.Json", "C#", "medium", "JSON library"),
    # Additional TypeScript
    Repo("koa", "https://github.com/koajs/koa", "JavaScript", "small", "Minimalist web framework"),
]

# Colors
CYAN = "\033[0;36m"
GREEN = "\033[0;32m"
YELLOW = "\033[1;33m"
RED = "\033[0;31m"
BLUE = "\033[0;34m"
NC = "\033[0m"


def log(msg: str) -> None:
    print(f"{BLUE}[bench]{NC} {msg}")


def log_success(msg: str) -> None:
    print(f"{GREEN}[bench]{NC} {msg}")


def log_error(msg: str) -> None:
    print(f"{RED}[bench]{NC} {msg}")


def get_local_codeix() -> str | None:
    """Get path to local build, or None if not found."""
    local_build = PROJECT_ROOT / "target" / "release" / "codeix"
    if local_build.exists():
        return str(local_build)
    return None


def get_codeix_bin() -> str | None:
    """Get codeix binary: CODEIX_BIN env var or local build. No PATH fallback."""
    from_env = os.environ.get("CODEIX_BIN")
    if from_env:
        return from_env
    return get_local_codeix()


def get_short_path(path: str) -> str:
    """Get a short relative path for display."""
    p = Path(path)
    # Try relative to PROJECT_ROOT
    try:
        return str(p.relative_to(PROJECT_ROOT))
    except ValueError:
        pass
    # Try relative to cwd
    try:
        return str(p.relative_to(Path.cwd()))
    except ValueError:
        pass
    # Just the filename
    return p.name


def build_index(codeix_bin: str, repo_path: Path) -> bool:
    """Build .codeindex for a single repo using the specified codeix binary.

    Returns True if index exists (already built or successfully built).
    """
    index_file = repo_path / ".codeindex" / "index.json"
    if index_file.exists():
        return True

    # Build command: either direct binary or npx
    if codeix_bin == "npx":
        cmd = ["npx", "codeix", "build", "-r", str(repo_path)]
    else:
        cmd = [codeix_bin, "build", "-r", str(repo_path)]

    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        log_error(f"  Failed to build index: {result.stderr[:200]}")
        return False
    return True


def clone_repo_to(repo: Repo, dest: Path) -> bool:
    """Clone a repo to a specific destination."""
    if dest.exists():
        return True

    dest.parent.mkdir(parents=True, exist_ok=True)

    for branch in ["main", "master", None]:
        cmd = ["git", "clone", "--depth=1", "--single-branch", "--quiet"]
        if branch:
            cmd.extend(["--branch", branch])
        cmd.extend([repo.url, str(dest)])

        result = subprocess.run(cmd, capture_output=True)
        if result.returncode == 0:
            break
    else:
        log_error(f"Failed to clone {repo.name}")
        return False

    # Shallow submodules if present (silently)
    gitmodules = dest / ".gitmodules"
    if gitmodules.exists():
        subprocess.run(
            ["git", "submodule", "update", "--init", "--depth=1", "--recursive", "--quiet"],
            cwd=dest,
            capture_output=True,
        )

    return True


def get_repo_by_name(name: str) -> Repo | None:
    """Get a Repo by name."""
    for repo in REPOS:
        if repo.name == name:
            return repo
    return None


def count_files(path: Path) -> int:
    """Count files in repo, excluding common build dirs."""
    exclude = {".git", "node_modules", "target", "__pycache__", ".venv"}
    count = 0
    for f in path.rglob("*"):
        if f.is_file() and not any(ex in f.parts for ex in exclude):
            count += 1
    return count


def count_lines(path: Path) -> int:
    """Count lines of code in common source files."""
    extensions = {".rs", ".py", ".js", ".ts", ".go", ".c", ".cpp", ".h", ".hpp", ".java", ".rb", ".zig", ".cs"}
    exclude = {".git", "node_modules", "target", "__pycache__", ".venv"}
    total = 0
    for f in path.rglob("*"):
        if f.is_file() and f.suffix in extensions and not any(ex in f.parts for ex in exclude):
            try:
                total += len(f.read_text(errors="ignore").splitlines())
            except Exception:
                pass
    return total


# ============================================================================
# Response Cache
# ============================================================================

def get_binary_version(bin_path: Path) -> str:
    """Get version string for a binary file (hash of contents)."""
    if bin_path.exists() and bin_path.is_file():
        return hashlib.sha256(bin_path.read_bytes()).hexdigest()[:12]
    return "unknown"


def get_npm_codeix_version() -> str:
    """Get version of codeix from npm registry."""
    try:
        result = subprocess.run(
            ["npm", "view", "codeix", "version"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except Exception:
        pass
    return "unknown"


def get_claude_version() -> str:
    """Get version of claude CLI."""
    try:
        result = subprocess.run(
            ["claude", "--version"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except Exception:
        pass
    return "unknown"


def get_cache_key_from_cmd(cmd: list[str]) -> str:
    """Generate cache key from command line.

    The command line IS the cache key. This works because:
    - Binary is named codeix-{version} (version embedded in filename)
    - cwd is the project repo (so paths are relative)
    - All params (--max-turns, prompt, etc.) are in the command
    """
    cmd_str = " ".join(cmd)
    return hashlib.sha256(cmd_str.encode()).hexdigest()[:16]


# Runtime errors that should invalidate cached responses
# These indicate transient failures, not actual API responses
# NOTE: Patterns must be specific enough to avoid false positives
RUNTIME_ERROR_PATTERNS = [
    "Separator is not found, and chunk exceed the limit",
    "Separator is found, but chunk is longer than limit",
    "Connection reset by peer",
    "Connection refused",
    "ECONNRESET",
    "ETIMEDOUT",
    "ECONNREFUSED",
    "read timeout",
    "connect timeout",
    "request timeout",
]


def _is_runtime_error(response: dict | list) -> bool:
    """Check if response contains a runtime error that should not be cached."""
    if not isinstance(response, dict):
        return False
    error = response.get("error")
    if not error:
        return False
    if not isinstance(error, str):
        return False
    for pattern in RUNTIME_ERROR_PATTERNS:
        if pattern.lower() in error.lower():
            return True
    return False


def get_cached_response(cache_key: str) -> dict | None:
    """Get cached response if exists and is valid.

    Rejects:
    - Corrupted JSON or missing response field
    - Runtime errors (transient failures that should be retried)
    """
    cache_file = RESPONSE_CACHE_DIR / f"{cache_key}.json"
    if cache_file.exists():
        try:
            data = json.loads(cache_file.read_text())
            if "response" not in data:
                cache_file.unlink()
                return None
            # Reject runtime errors (transient failures)
            if _is_runtime_error(data["response"]):
                cache_file.unlink()
                return None
            return data
        except (json.JSONDecodeError, OSError):
            cache_file.unlink()
            return None
    return None


def delete_cached_response(cache_key: str) -> None:
    """Delete a cached response."""
    cache_file = RESPONSE_CACHE_DIR / f"{cache_key}.json"
    if cache_file.exists():
        cache_file.unlink()


def save_cached_response(cache_key: str, response: dict, metadata: dict | None = None) -> None:
    """Save response to cache.

    Does NOT cache runtime errors (transient failures that should be retried).
    """
    # Don't cache runtime errors
    if _is_runtime_error(response):
        return

    RESPONSE_CACHE_DIR.mkdir(parents=True, exist_ok=True)
    cache_file = RESPONSE_CACHE_DIR / f"{cache_key}.json"
    data = {"response": response}
    if metadata:
        data["metadata"] = metadata
    cache_file.write_text(json.dumps(data, indent=2))
