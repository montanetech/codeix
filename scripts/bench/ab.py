"""A/B benchmark framework using claude CLI."""

import asyncio
import atexit
import json
import os
import re
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from .common import (
    CYAN,
    NC,
    RunContext,
    create_run_context,
    delete_cached_response,
    get_cache_key_from_cmd,
    get_cached_response,
    log,
    log_error,
    log_success,
    save_cached_response,
)

# Max parallel questions (each runs 2 Claude calls + 1 judge)
MAX_PARALLEL_QUESTIONS = int(os.environ.get("CODEIX_BENCH_PARALLEL", 4))


def build_codeix_cmd(
    mcp_config: str,
    prompt: str,
    max_turns: int = 25,
) -> list[str]:
    """Build a claude command with codeix MCP tools.

    Standardized arg order ensures cache key consistency across benchmarks.
    """
    return [
        "claude", "--print", "--output-format", "json",
        "--no-session-persistence",
        "--max-turns", str(max_turns),
        "--allowedTools", "mcp__codeindex__*",
        "--mcp-config", mcp_config,
        "-p", prompt,
    ]


def build_claude_cmd(
    prompt: str,
    max_turns: int = 25,
) -> list[str]:
    """Build a raw claude command (no tools)."""
    return [
        "claude", "--print", "--output-format", "json",
        "--no-session-persistence",
        "--max-turns", str(max_turns),
        "-p", prompt,
    ]


def build_mcp_config(bin_path: str) -> str:
    """Build MCP config JSON for codeix.

    Uses "." as repo path since cwd is set to the project repo.
    This makes the command line consistent across runs (for caching).
    """
    return json.dumps({
        "mcpServers": {
            "codeindex": {
                "command": bin_path,
                "args": ["serve", "."],
            }
        }
    })


def build_prompt(project: str, question: str) -> str:
    """Build standardized prompt for codeix questions."""
    return f"Project: {project}\n\n{question}\n\nUse the codeindex MCP tools to answer."


@dataclass
class ABConfig:
    """Configuration for A/B benchmark."""
    name: str
    label_a: str
    label_b: str
    title: str
    # Called once at start to setup binaries, returns (bin_a, bin_b)
    # bin_a/bin_b are versioned names (e.g., "codeix-abc123", "codeix-0.2.0")
    setup_run: Callable[[RunContext], tuple[str, str]]
    # Returns (cmd_a, cwd_a, cmd_b, cwd_b) for a question
    # cwd is set to the project repo so paths are relative
    get_commands: Callable[[dict, RunContext], tuple[list[str], Path, list[str], Path]]
    # Optional per-question setup callbacks: setup_a(question, ctx) -> bool
    setup_a: Callable[[dict, RunContext], bool] | None = None
    setup_b: Callable[[dict, RunContext], bool] | None = None
    # Extra judge output fields
    extra_judge_fields: str = ""


def parse_judge_winner(judge: dict) -> str:
    """Extract winner from judge response."""
    if not isinstance(judge, dict):
        return "?"
    # With --json-schema, output is in structured_output
    structured = judge.get("structured_output", {})
    if isinstance(structured, dict) and "winner" in structured:
        return structured["winner"]
    # Fallback: try result field (for legacy cached responses)
    result_text = judge.get("result", "")
    if isinstance(result_text, dict):
        return result_text.get("winner", "?")
    if isinstance(result_text, str):
        # Try direct JSON parse
        try:
            return json.loads(result_text).get("winner", "?")
        except json.JSONDecodeError:
            pass
        # Extract JSON from markdown (handles nested braces better)
        match = re.search(r'\{[^{}]*"winner"\s*:\s*"([^"]+)"[^{}]*\}', result_text)
        if match:
            return match.group(1)
        # Fallback: look for "winner": "X" pattern anywhere
        match = re.search(r'"winner"\s*:\s*"([^"]+)"', result_text, re.IGNORECASE)
        if match:
            return match.group(1)
        # Fallback: look for "Winner: X" or "**Winner**: X" in text
        match = re.search(r'\*?\*?[Ww]inner\*?\*?\s*:\s*\*?\*?([ABab]|[Tt]ie)\*?\*?', result_text)
        if match:
            return match.group(1).upper() if match.group(1).upper() in ("A", "B") else "tie"
    return "?"


async def run_subprocess(cmd: list[str], cwd: Path | None = None, bin_dir: Path | None = None) -> dict:
    """Run a subprocess command without TTY (prevents terminal issues with claude).

    If bin_dir is provided, it's prepended to PATH so versioned binaries can be found.
    """
    import subprocess

    # Build environment with bin_dir in PATH
    env = os.environ.copy()
    if bin_dir:
        env["PATH"] = f"{bin_dir}:{env.get('PATH', '')}"

    try:
        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdin=asyncio.subprocess.DEVNULL,  # No TTY - prevents terminal issues
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            cwd=str(cwd) if cwd else None,
            env=env,
            start_new_session=True,  # Detach from controlling terminal
        )
        stdout, stderr = await proc.communicate()
        stdout_str = stdout.decode() if stdout else ""
        stderr_str = stderr.decode() if stderr else ""

        try:
            return json.loads(stdout_str) if stdout_str else {"result": "", "error": stderr_str}
        except json.JSONDecodeError:
            return {"result": stdout_str, "error": stderr_str}
    except asyncio.CancelledError:
        proc.terminate()
        raise
    except Exception as e:
        return {"result": "", "error": str(e)}


async def run_question(
    q: dict,
    config: ABConfig,
    ctx: RunContext,
) -> dict | None:
    """Run A/B test for a single question.

    Cache key = command line. This works because:
    - Binary is named codeix-{version} (version embedded in filename)
    - cwd is the project repo (so paths are relative)
    - All params (--max-turns, prompt, etc.) are in the command
    """
    # Get commands first (needed for cache keys)
    # This is cheap - just builds the command strings
    cmd_a, cwd_a, cmd_b, cwd_b = config.get_commands(q, ctx)

    # Cache key = command line
    cache_key_a = get_cache_key_from_cmd(cmd_a)
    cache_key_b = get_cache_key_from_cmd(cmd_b)

    cached_a = get_cached_response(cache_key_a)
    cached_b = get_cached_response(cache_key_b)

    # If both cached, skip setup entirely
    if cached_a and cached_b:
        log(f"  Cache hit: {q['id']} ({q['category']})")
        response_a = cached_a["response"]
        response_b = cached_b["response"]
    else:
        # Only setup (clone + build) if we need to run
        if not cached_a and config.setup_a and not config.setup_a(q, ctx):
            log_error(f"  Setup A failed for {q['id']}")
            return None
        if not cached_b and config.setup_b and not config.setup_b(q, ctx):
            log_error(f"  Setup B failed for {q['id']}")
            return None

        # Run A and B in parallel (only if not cached)
        tasks = []
        if not cached_a:
            tasks.append(asyncio.create_task(run_subprocess(cmd_a, cwd_a, ctx.bin_dir)))
        if not cached_b:
            tasks.append(asyncio.create_task(run_subprocess(cmd_b, cwd_b, ctx.bin_dir)))

        if tasks:
            results = await asyncio.gather(*tasks)
            idx = 0
            if not cached_a:
                response_a = results[idx]
                idx += 1
            else:
                response_a = cached_a["response"]
            if not cached_b:
                response_b = results[idx]
            else:
                response_b = cached_b["response"]
        else:
            response_a = cached_a["response"]
            response_b = cached_b["response"]

        # Save to cache
        if not cached_a:
            save_cached_response(cache_key_a, response_a, {"question_id": q["id"], "label": config.label_a})
        if not cached_b:
            save_cached_response(cache_key_b, response_b, {"question_id": q["id"], "label": config.label_b})

    # Helper to check for errors
    def has_error(resp: dict) -> str | None:
        # Check subtype first - more reliable than is_error
        subtype = resp.get("subtype", "")
        if subtype.startswith("error"):
            return subtype
        # Check for rate limit in result
        result = resp.get("result", "")
        if isinstance(result, str) and "hit your limit" in result.lower():
            return "rate_limit"
        # is_error with non-success subtype
        if resp.get("is_error") and subtype != "success":
            return subtype or "error"
        return None

    # Check for errors and log them
    error_a = has_error(response_a)
    error_b = has_error(response_b)
    if error_a:
        log_error(f"  A error: {error_a}")
    if error_b:
        log_error(f"  B error: {error_b}")

    # Skip judging if either response has an error
    if error_a or error_b:
        result = {
            "question": q,
            "response_a": response_a,
            "response_b": response_b,
            "judge": {},
            "cost_a": response_a.get("total_cost_usd"),
            "cost_b": response_b.get("total_cost_usd"),
            "cached_a": cached_a is not None,
            "cached_b": cached_b is not None,
            "cached_judge": False,
            "error_a": error_a,
            "error_b": error_b,
        }
        result_file = ctx.results_dir / f"{q['id']}.json"
        result_file.write_text(json.dumps(result, indent=2))
        log(f"  Done: {q['id']} (skipped judge due to error)")
        return result

    # Judge using subprocess (with caching)
    # Truncate responses for judge prompt
    response_a_text = json.dumps(response_a.get('result', response_a), indent=2)[:2000]
    response_b_text = json.dumps(response_b.get('result', response_b), indent=2)[:2000]

    # Get costs for judge evaluation
    cost_a = response_a.get("total_cost_usd")
    cost_b = response_b.get("total_cost_usd")
    cost_info = ""
    if cost_a is not None and cost_b is not None:
        cost_info = f"\n\nCOST:\nA: ${cost_a:.4f}\nB: ${cost_b:.4f}"

    judge_prompt = f"""Compare these two responses to the question: "{q['question']}"

RESPONSE A ({config.label_a}):
{response_a_text}

RESPONSE B ({config.label_b}):
{response_b_text}{cost_info}

Evaluate:
1. Accuracy - Which response is more correct?
2. Completeness - Which found more relevant information?
3. Efficiency - Consider cost (lower is better for similar quality)

Output JSON: {{"winner": "A"|"B"|"tie", "reason": "brief explanation"{config.extra_judge_fields}}}"""

    # Build judge command (for cache key)
    judge_schema = '{"type":"object","properties":{"winner":{"type":"string","enum":["A","B","tie"]},"reason":{"type":"string"}},"required":["winner","reason"]}'
    judge_cmd = [
        "claude", "--print", "--output-format", "json",
        "--no-session-persistence",
        "--max-turns", "15",
        "--json-schema", judge_schema,
        "-p", judge_prompt,
    ]
    judge_cache_key = get_cache_key_from_cmd(judge_cmd)
    cached_judge = get_cached_response(judge_cache_key)

    # Validate judge response contains a winner, bust cache if not
    if cached_judge:
        response = cached_judge.get("response", {})
        structured = response.get("structured_output", {})
        result_text = response.get("result", "")
        # Valid if structured_output has winner, or result has winner (legacy)
        has_winner = (isinstance(structured, dict) and "winner" in structured) or (result_text and '"winner"' in result_text)
        if not has_winner:
            delete_cached_response(judge_cache_key)
            cached_judge = None

    if cached_judge:
        judge_response = cached_judge["response"]
        cached_judge_flag = True
    else:
        # Run judge via subprocess (no tools needed)
        judge_response = await run_subprocess(judge_cmd)
        # With --json-schema, output is in structured_output field
        structured = judge_response.get("structured_output", {})
        if structured and "winner" in structured:
            save_cached_response(judge_cache_key, judge_response, {"question_id": q["id"], "type": "judge"})
        else:
            # Debug: log when judge fails to return winner
            error_text = judge_response.get("error", "")
            log_error(f"Judge failed for {q['id']}: structured={structured}, error={error_text[:200] if error_text else 'none'}")
        cached_judge_flag = False

    result = {
        "question": q,
        "response_a": response_a,
        "response_b": response_b,
        "judge": judge_response,
        "cost_a": response_a.get("total_cost_usd"),
        "cost_b": response_b.get("total_cost_usd"),
        "cached_a": cached_a is not None,
        "cached_b": cached_b is not None,
        "cached_judge": cached_judge_flag,
        "error_a": None,  # No errors if we got here (errors returned early above)
        "error_b": None,
    }

    result_file = ctx.results_dir / f"{q['id']}.json"
    result_file.write_text(json.dumps(result, indent=2))
    log(f"  Done: {q['id']}")

    return result


async def run_async(
    config: ABConfig,
    question_id: str | None = None,
) -> list[dict]:
    """Run A/B benchmark with given configuration (async). Returns results list."""
    questions_file = Path(__file__).parent / "questions.json"

    if not questions_file.exists():
        log_error(f"Questions file not found: {questions_file}")
        sys.exit(1)

    questions = json.loads(questions_file.read_text())
    if question_id:
        questions = [q for q in questions if q["id"] == question_id]
        if not questions:
            log_error(f"Question '{question_id}' not found")
            sys.exit(1)

    # Create temporary run directory with standard structure
    ctx = create_run_context()

    def cleanup():
        shutil.rmtree(ctx.run_dir, ignore_errors=True)
    atexit.register(cleanup)

    log(f"Running {config.name} with {len(questions)} question(s) in parallel")
    log(f"Run dir: {ctx.run_dir}")
    print()

    # Setup binaries (once, before running questions)
    # Binaries are named with version embedded (e.g., codeix-abc123)
    bin_a, bin_b = config.setup_run(ctx)
    log(f"A: {config.label_a} ({bin_a})")
    log(f"B: {config.label_b} ({bin_b})")

    # Create semaphore to limit parallelism
    sem = asyncio.Semaphore(MAX_PARALLEL_QUESTIONS)

    async def run_with_sem(q: dict) -> dict | None:
        async with sem:
            return await run_question(q, config, ctx)

    # Run all questions with limited parallelism
    tasks = [asyncio.create_task(run_with_sem(q)) for q in questions]

    try:
        results = await asyncio.gather(*tasks)
        results = [r for r in results if r is not None]
    except asyncio.CancelledError:
        # Cancel all pending tasks
        for task in tasks:
            task.cancel()
        print("\n\nInterrupted, stopping...")
        print("Aborted.")
        sys.exit(130)

    # Summary
    print()
    print(f"{CYAN}═══════════════════════════════════════════════════════════════{NC}")
    print(f"{CYAN}{config.title:^63}{NC}")
    print(f"{CYAN}═══════════════════════════════════════════════════════════════{NC}")
    print()
    # Show what A and B represent
    print(f"A: {config.label_a}")
    print(f"B: {config.label_b}")
    print()
    print(f"{'Question':<30} {'Winner':^8}  {'A cost':>8}  {'B cost':>8}")
    print("─" * 60)

    wins = {"A": 0, "B": 0, "tie": 0}
    total_cost_a = 0.0
    total_cost_b = 0.0
    errors = 0
    for r in results:
        winner = parse_judge_winner(r.get("judge", {}))
        wins[winner] = wins.get(winner, 0) + 1

        if r['cost_a']:
            total_cost_a += r['cost_a']
        if r['cost_b']:
            total_cost_b += r['cost_b']

        cost_a = f"${r['cost_a']:.2f}" if r['cost_a'] else "-"
        cost_b = f"${r['cost_b']:.2f}" if r['cost_b'] else "-"

        # Show errors in output
        error_info = ""
        if r.get('error_a') or r.get('error_b'):
            errors += 1
            error_parts = []
            if r.get('error_a'):
                error_parts.append(f"A:{r['error_a']}")
            if r.get('error_b'):
                error_parts.append(f"B:{r['error_b']}")
            error_info = f"  [{', '.join(error_parts)}]"

        print(f"{r['question']['id']:<30} {winner:^8}  {cost_a:>8}  {cost_b:>8}{error_info}")

    # Summary row
    print("─" * 60)
    a_wins = wins.get('A', 0)
    b_wins = wins.get('B', 0)
    wins_summary = f"A:{a_wins} B:{b_wins}"
    cost_a_str = f"${total_cost_a:.2f}"
    cost_b_str = f"${total_cost_b:.2f}"
    print(f"{'TOTAL':<30} {wins_summary:^8}  {cost_a_str:>8}  {cost_b_str:>8}")

    print()
    if a_wins > b_wins:
        overall = f"A ({config.label_a})"
    elif b_wins > a_wins:
        overall = f"B ({config.label_b})"
    else:
        overall = "tie"
    print(f"Winner: {overall}")

    # Cache stats
    cache_hits_a = sum(1 for r in results if r.get("cached_a"))
    cache_hits_b = sum(1 for r in results if r.get("cached_b"))
    cache_hits_judge = sum(1 for r in results if r.get("cached_judge"))
    if cache_hits_a or cache_hits_b or cache_hits_judge:
        print(f"Cache: A={cache_hits_a}/{len(results)}, B={cache_hits_b}/{len(results)}, judge={cache_hits_judge}/{len(results)}")

    return results


def run(
    config: ABConfig,
    question_id: str | None = None,
) -> list[dict]:
    """Run A/B benchmark with given configuration. Returns results list."""
    try:
        return asyncio.run(run_async(config, question_id))
    except KeyboardInterrupt:
        print("\n\nInterrupted.")
        sys.exit(130)
