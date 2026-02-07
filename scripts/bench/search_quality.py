"""Search quality benchmark: dev codeix vs prod codeix."""

import shutil
from pathlib import Path

from .ab import (
    ABConfig,
    build_codeix_cmd,
    build_mcp_config,
    build_prompt,
    run as run_ab,
)
from .common import (
    RunContext,
    clone_repo_to,
    build_index,
    get_binary_version,
    get_codeix_bin,
    get_npm_codeix_version,
    get_repo_by_name,
)


def run(question_id: str | None = None) -> list[dict]:
    """Run A/B: dev codeix vs prod codeix. Returns results list."""

    def setup_run(ctx: RunContext) -> tuple[str, str]:
        """Setup binaries in run dir, return (bin_a, bin_b).

        Binaries are named with version embedded for cache key stability:
        - codeix-{hash} for local dev build
        - codeix-{version} for npm package
        """
        # A: copy local dev binary with version in name
        dev_src = get_codeix_bin()
        if not dev_src:
            raise RuntimeError("No local codeix build found. Run 'cargo build --release' first.")
        version_a = get_binary_version(Path(dev_src))
        bin_a = ctx.bin_dir / f"codeix-{version_a}"
        shutil.copy2(dev_src, bin_a)
        bin_a.chmod(0o755)

        # B: wrapper script for npx codeix, with version in name
        version_b = get_npm_codeix_version()
        bin_b = ctx.bin_dir / f"codeix-{version_b}"
        bin_b.write_text("#!/bin/sh\nexec npx codeix \"$@\"\n")
        bin_b.chmod(0o755)

        return str(bin_a), str(bin_b)

    def setup_a(q: dict, ctx: RunContext) -> bool:
        """Clone repo and build index for A."""
        repo = get_repo_by_name(q["project"])
        if not repo:
            return False
        dest = ctx.repos_a / q["project"]
        if not clone_repo_to(repo, dest):
            return False
        # Find the versioned binary (there's only one codeix-* in bin_dir for A)
        bin_a = next(ctx.bin_dir.glob("codeix-*"))
        return build_index(str(bin_a), dest)

    def setup_b(q: dict, ctx: RunContext) -> bool:
        """Clone repo and build index for B."""
        repo = get_repo_by_name(q["project"])
        if not repo:
            return False
        dest = ctx.repos_b / q["project"]
        if not clone_repo_to(repo, dest):
            return False
        # Find the versioned binary for B (the npm version one)
        version_b = get_npm_codeix_version()
        bin_b = ctx.bin_dir / f"codeix-{version_b}"
        return build_index(str(bin_b), dest)

    def get_commands(q: dict, ctx: RunContext) -> tuple[list[str], Path, list[str], Path]:
        """Generate commands for A and B.

        Returns (cmd_a, cwd_a, cmd_b, cwd_b).
        cwd is set to project repo so paths are relative (for cache key stability).
        Binary paths use just the versioned name (e.g., "codeix-abc123") for stable cache keys.
        """
        # Find versioned binaries - use just the name, not full path
        dev_src = get_codeix_bin()
        version_a = get_binary_version(Path(dev_src)) if dev_src else "unknown"
        version_b = get_npm_codeix_version()

        # Use just the binary name - we'll set PATH to include bin_dir
        bin_name_a = f"codeix-{version_a}"
        bin_name_b = f"codeix-{version_b}"

        cwd_a = ctx.repos_a / q["project"]
        cwd_b = ctx.repos_b / q["project"]

        prompt = build_prompt(q["project"], q["question"])
        # MCP config uses versioned binary name (PATH will be set to find it)
        cmd_a = build_codeix_cmd(build_mcp_config(bin_name_a), prompt)
        cmd_b = build_codeix_cmd(build_mcp_config(bin_name_b), prompt)

        return cmd_a, cwd_a, cmd_b, cwd_b

    config = ABConfig(
        name="search-quality benchmark",
        label_a="codeix-dev",
        label_b="codeix-prod",
        title="SEARCH QUALITY BENCHMARK",
        setup_run=setup_run,
        get_commands=get_commands,
        setup_a=setup_a,
        setup_b=setup_b,
    )
    return run_ab(config, question_id)
