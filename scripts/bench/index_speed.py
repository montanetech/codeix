"""Quantitative indexing speed benchmark."""

import atexit
import shutil
import subprocess
import sys
import time

from .common import (
    CYAN,
    NC,
    REPOS,
    RunContext,
    clone_repo_to,
    count_files,
    count_lines,
    create_run_context,
    get_codeix_bin,
    get_repo_by_name,
    log,
    log_error,
    log_success,
)


def benchmark_repo(repo, ctx: RunContext, codeix_bin: str, verbose: bool = False) -> dict | None:
    """Benchmark indexing a single repo."""
    path = ctx.repos / repo.name
    if not path.exists():
        log_error(f"Directory not found: {path}")
        return None

    files = count_files(path)
    lines = count_lines(path)

    # Clean existing index
    index_dir = path / ".codeindex"
    if index_dir.exists():
        shutil.rmtree(index_dir)

    # Benchmark
    start = time.perf_counter()
    result = subprocess.run(
        [codeix_bin, "build", str(path)],
        capture_output=not verbose,
    )
    duration = time.perf_counter() - start

    if result.returncode != 0:
        log_error(f"Failed to index {repo.name}")
        return None

    files_per_sec = int(files / duration) if duration > 0 else 0

    return {
        "name": repo.name,
        "lang": repo.lang,
        "size": repo.size,
        "files": files,
        "lines": lines,
        "duration": duration,
        "files_per_sec": files_per_sec,
        "notes": repo.notes,
    }


def run(verbose: bool = False) -> None:
    """Run quantitative indexing benchmark."""
    print()
    print(f"{CYAN}╔══════════════════════════════════════════════════════════════════════════════════════════════════╗{NC}")
    print(f"{CYAN}║                                   CODEIX BENCHMARK                                               ║{NC}")
    print(f"{CYAN}╚══════════════════════════════════════════════════════════════════════════════════════════════════╝{NC}")
    print()

    # Check codeix - local build or CODEIX_BIN only
    codeix_bin = get_codeix_bin()
    if not codeix_bin:
        log_error("codeix not found (build with 'cargo build --release' or set CODEIX_BIN)")
        sys.exit(1)
    log(f"Using codeix: {codeix_bin}")

    # Create fresh run context
    ctx = create_run_context()
    log(f"Run dir: {ctx.run_dir}")

    def cleanup():
        shutil.rmtree(ctx.run_dir, ignore_errors=True)
    atexit.register(cleanup)

    # Copy codeix binary to run dir for reproducibility
    bin_path = ctx.bin_dir / "codeix"
    shutil.copy2(codeix_bin, bin_path)
    bin_path.chmod(0o755)
    codeix_bin = str(bin_path)

    # Clone phase
    print()
    log("Phase 1: Cloning repositories...")
    print()

    start = time.perf_counter()
    for repo in REPOS:
        clone_repo_to(repo, ctx.repos / repo.name)
    clone_duration = time.perf_counter() - start

    log_success(f"Cloning complete in {clone_duration:.1f}s")

    # Benchmark phase
    print()
    log("Phase 2: Benchmarking indexing...")
    print()

    # Header
    print(f"{'Repository':<18} │ {'Language':<10} │ {'Size':<7} │ {'Files':>6} │ {'Lines':>8} │ {'Time':>7} │ {'Speed':>8} │ Notes")
    print("───────────────────┼────────────┼─────────┼────────┼──────────┼─────────┼──────────┼─────────────────────────")

    results = []
    for repo in REPOS:
        result = benchmark_repo(repo, ctx, codeix_bin=codeix_bin, verbose=verbose)
        if result:
            results.append(result)
            print(
                f"{result['name']:<18} │ {result['lang']:<10} │ {result['size']:<7} │ "
                f"{result['files']:>6} │ {result['lines']:>8} │ {result['duration']:>6.2f}s │ "
                f"{result['files_per_sec']:>6}/s │ {result['notes']}"
            )
        else:
            print(f"{repo.name:<18} │ {repo.lang:<10} │ {repo.size:<7} │ {'FAILED':>6} │ {'-':>8} │ {'-':>7} │ {'-':>8} │ {repo.notes}")

    print()
    log_success("Benchmark complete!")
    print()
    print(f"{CYAN}Languages tested:{NC} TypeScript, Go, C++, Rust, Python, Java, C, Ruby, C#, JavaScript")
    print(f"{CYAN}Structural tests:{NC} Small/Medium repos, with/without submodules")
    print()
