"""Entry point for python -m scripts.bench."""

import argparse
import json
import shutil
import sys
from datetime import datetime
from pathlib import Path

from . import index_speed, search_quality, search_value
from .ab import parse_judge_winner
from .common import RESPONSE_CACHE_DIR, log, log_success


def export_results(results: list[dict], config_name: str, output_path: str | None = None) -> None:
    """Export benchmark results in pytest-like JSON format.

    If output_path is None, writes to stdout (and normal output goes to stderr).
    """
    if not results:
        log("No results to export")
        return

    tests = []
    summary = {
        "a_wins": 0,
        "b_wins": 0,
        "ties": 0,
        "unknown": 0,
        "total": len(results),
        "a": {
            "total_cost": 0.0,
            "total_tokens": {"input": 0, "output": 0},
            "total_tool_calls": 0,
        },
        "b": {
            "total_cost": 0.0,
            "total_tokens": {"input": 0, "output": 0},
            "total_tool_calls": 0,
        },
    }

    for r in results:
        q = r.get("question", {})
        winner = parse_judge_winner(r.get("judge", {}))

        if winner == "A":
            summary["a_wins"] += 1
        elif winner == "B":
            summary["b_wins"] += 1
        elif winner == "tie":
            summary["ties"] += 1
        else:
            summary["unknown"] += 1

        # Aggregate costs
        if r.get("cost_a"):
            summary["a"]["total_cost"] += r["cost_a"]
        if r.get("cost_b"):
            summary["b"]["total_cost"] += r["cost_b"]

        # Aggregate tokens
        usage_a = r.get("usage_a", {})
        usage_b = r.get("usage_b", {})
        summary["a"]["total_tokens"]["input"] += usage_a.get("input_tokens", 0)
        summary["a"]["total_tokens"]["output"] += usage_a.get("output_tokens", 0)
        summary["b"]["total_tokens"]["input"] += usage_b.get("input_tokens", 0)
        summary["b"]["total_tokens"]["output"] += usage_b.get("output_tokens", 0)

        # Aggregate tool calls
        tool_usage_a = r.get("tool_usage_a", {})
        tool_usage_b = r.get("tool_usage_b", {})
        summary["a"]["total_tool_calls"] += tool_usage_a.get("tool_count", 0)
        summary["b"]["total_tool_calls"] += tool_usage_b.get("tool_count", 0)

        tests.append({
            "nodeid": f"{config_name}:{q.get('id', 'unknown')}",
            "question": q.get("question"),
            "category": q.get("category"),
            "project": q.get("project"),
            "winner": winner,
            "judge_reason": _extract_reason(r.get("judge", {})),
            "a": {
                "cost": r.get("cost_a"),
                "usage": usage_a,
                "tool_usage": tool_usage_a,
                "turns": r.get("turns_a", []),
                "cached": r.get("cached_a"),
                "response": r.get("response_a", {}).get("result"),
            },
            "b": {
                "cost": r.get("cost_b"),
                "usage": usage_b,
                "tool_usage": tool_usage_b,
                "turns": r.get("turns_b", []),
                "cached": r.get("cached_b"),
                "response": r.get("response_b", {}).get("result"),
            },
        })

    report = {
        "created": datetime.now().isoformat(),
        "benchmark": config_name,
        "summary": summary,
        "tests": tests,
    }

    if output_path:
        output = Path(output_path)
        output.write_text(json.dumps(report, indent=2))
        log_success(f"Exported {len(results)} results to {output}")
    else:
        # Write directly to stdout (bypass any print redirection)
        sys.stdout.write(json.dumps(report, indent=2))
        sys.stdout.write("\n")
        sys.stdout.flush()


def _extract_field(judge: dict, field: str) -> str | None:
    """Extract a field from judge response."""
    if not isinstance(judge, dict):
        return None
    # With --json-schema, output is in structured_output
    structured = judge.get("structured_output", {})
    if isinstance(structured, dict) and field in structured:
        return structured[field]
    # Fallback: try result field (for legacy cached responses)
    result_text = judge.get("result", "")
    if isinstance(result_text, dict):
        return result_text.get(field)
    if isinstance(result_text, str):
        # Try direct JSON parse
        try:
            return json.loads(result_text).get(field)
        except json.JSONDecodeError:
            pass
        # Extract from markdown JSON block
        import re
        match = re.search(rf'"{field}"\s*:\s*"([^"]+)"', result_text)
        if match:
            return match.group(1)
    return None


def _extract_reason(judge: dict) -> str | None:
    """Extract reason from judge response."""
    return _extract_field(judge, "reason")




def main() -> None:
    parser = argparse.ArgumentParser(
        description="Codeix Benchmark Suite",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Commands:
  index-speed      Quantitative indexing speed benchmark
  search-quality   A/B: dev codeix vs prod codeix
  search-value     A/B: codeix vs raw Claude (no MCP)
  clear-cache      Clear response cache

Examples:
  python -m scripts.bench index-speed
  python -m scripts.bench search-quality
  python -m scripts.bench search-value
  python -m scripts.bench search-value --question entry-point
  python -m scripts.bench search-quality --export results.json
  python -m scripts.bench search-value --export  # exports to stdout
  python -m scripts.bench clear-cache
""",
    )
    parser.add_argument("command", choices=["index-speed", "search-quality", "search-value", "clear-cache"], help="Benchmark command")
    parser.add_argument("--verbose", "-v", action="store_true", help="Show detailed output")
    parser.add_argument("--question", "-q", help="Run specific question by ID")
    parser.add_argument("--export", "-e", nargs="?", const="-", metavar="FILE", help="Export results to JSON file, or stdout if no file given")

    args = parser.parse_args()

    # If exporting to stdout, redirect normal output to stderr
    export_to_stdout = args.export == "-"
    if export_to_stdout:
        # Monkey-patch print to go to stderr during benchmark run
        import builtins
        _original_print = builtins.print
        builtins.print = lambda *a, **kw: _original_print(*a, **kw, file=sys.stderr)

    if args.command == "index-speed":
        index_speed.run(args.verbose)
    elif args.command == "search-quality":
        results = search_quality.run(args.question)
        if args.export:
            export_results(results, "search-quality", None if export_to_stdout else args.export)
    elif args.command == "search-value":
        results = search_value.run(args.question)
        if args.export:
            export_results(results, "search-value", None if export_to_stdout else args.export)
    elif args.command == "clear-cache":
        if RESPONSE_CACHE_DIR.exists():
            shutil.rmtree(RESPONSE_CACHE_DIR)
            log_success(f"Cleared response cache: {RESPONSE_CACHE_DIR}")
        else:
            log("Response cache already empty")


if __name__ == "__main__":
    main()
