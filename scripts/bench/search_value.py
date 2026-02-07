"""Search value benchmark: codeix vs raw Claude."""

import shutil
from pathlib import Path

from .ab import (
    ABConfig,
    build_claude_cmd,
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
    get_claude_version,
    get_repo_by_name,
)


def run(question_id: str | None = None) -> list[dict]:
    """Run A/B: codeix vs raw Claude. Returns results list."""

    def setup_run(ctx: RunContext) -> tuple[str, str]:
        """Setup binaries in run dir, return (bin_a, bin_b).

        Binaries are named with version embedded for cache key stability:
        - codeix-{hash} for local dev build
        - claude (version injected in prompt for B)
        """
        # A: copy local dev binary with version in name
        dev_src = get_codeix_bin()
        if not dev_src:
            raise RuntimeError("No local codeix build found. Run 'cargo build --release' first.")
        version_a = get_binary_version(Path(dev_src))
        bin_a = ctx.bin_dir / f"codeix-{version_a}"
        shutil.copy2(dev_src, bin_a)
        bin_a.chmod(0o755)

        # B: raw Claude (version will be injected in prompt)
        bin_b = "claude"

        return str(bin_a), bin_b

    def setup_a(q: dict, ctx: RunContext) -> bool:
        """Clone repo and build index for A."""
        repo = get_repo_by_name(q["project"])
        if not repo:
            return False
        dest = ctx.repos_a / q["project"]
        if not clone_repo_to(repo, dest):
            return False
        # Find the versioned binary
        dev_src = get_codeix_bin()
        version_a = get_binary_version(Path(dev_src)) if dev_src else "unknown"
        bin_a = ctx.bin_dir / f"codeix-{version_a}"
        return build_index(str(bin_a), dest)

    def setup_b(q: dict, ctx: RunContext) -> bool:
        """Clone repo for B (raw Claude needs files to read)."""
        repo = get_repo_by_name(q["project"])
        if not repo:
            return False
        dest = ctx.repos_b / q["project"]
        return clone_repo_to(repo, dest)

    def get_commands(q: dict, ctx: RunContext) -> tuple[list[str], Path, list[str], Path]:
        """Generate commands for A and B.

        Returns (cmd_a, cwd_a, cmd_b, cwd_b).
        cwd is set to project repo so paths are relative (for cache key stability).
        """
        # Find versioned binaries
        dev_src = get_codeix_bin()
        version_a = get_binary_version(Path(dev_src)) if dev_src else "unknown"
        version_b = get_claude_version()

        # Use just the binary name - PATH will include bin_dir
        bin_name_a = f"codeix-{version_a}"

        cwd_a = ctx.repos_a / q["project"]
        cwd_b = ctx.repos_b / q["project"]

        prompt = build_prompt(q["project"], q["question"])
        # MCP config uses versioned binary name (PATH will be set to find it)
        cmd_a = build_codeix_cmd(build_mcp_config(bin_name_a), prompt)
        # B: inject claude version in prompt for cache busting
        prompt_b = f"[claude:{version_b}] {q['question']}"
        cmd_b = build_claude_cmd(prompt_b)

        return cmd_a, cwd_a, cmd_b, cwd_b

    config = ABConfig(
        name="search-value benchmark",
        label_a="codeix-dev",
        label_b="claude",
        title="CODEIX VALUE BENCHMARK",
        setup_run=setup_run,
        get_commands=get_commands,
        setup_a=setup_a,
        setup_b=setup_b,
        extra_judge_fields=', "codeix_value": "high|medium|low|none"',
    )
    return run_ab(config, question_id)
