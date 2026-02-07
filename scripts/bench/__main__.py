"""Entry point for python -m scripts.bench."""

import argparse
import json
import shutil
from datetime import datetime
from pathlib import Path

from . import index_speed, search_quality, search_value
from .ab import parse_judge_winner
from .common import RESPONSE_CACHE_DIR, log, log_success


def export_results(results: list[dict], config_name: str, output_path: str) -> None:
    """Export benchmark results in pytest-like JSON format."""
    if not results:
        log("No results to export")
        return

    tests = []
    summary = {"a_wins": 0, "b_wins": 0, "ties": 0, "unknown": 0, "total": len(results)}

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

        tests.append({
            "nodeid": f"{config_name}:{q.get('id', 'unknown')}",
            "category": q.get("category"),
            "project": q.get("project"),
            "winner": winner,
            "cost_a": r.get("cost_a"),
            "cost_b": r.get("cost_b"),
            "cached_a": r.get("cached_a"),
            "cached_b": r.get("cached_b"),
            "judge_reason": _extract_reason(r.get("judge", {})),
            "response_a": r.get("response_a", {}).get("result"),
            "response_b": r.get("response_b", {}).get("result"),
        })

    report = {
        "created": datetime.now().isoformat(),
        "benchmark": config_name,
        "summary": summary,
        "tests": tests,
    }

    output = Path(output_path)
    output.write_text(json.dumps(report, indent=2))
    log_success(f"Exported {len(results)} results to {output}")


def _extract_reason(judge: dict) -> str | None:
    """Extract reason from judge response."""
    if not isinstance(judge, dict):
        return None
    # With --json-schema, output is in structured_output
    structured = judge.get("structured_output", {})
    if isinstance(structured, dict) and "reason" in structured:
        return structured["reason"]
    # Fallback: try result field (for legacy cached responses)
    result_text = judge.get("result", "")
    if isinstance(result_text, dict):
        return result_text.get("reason")
    if isinstance(result_text, str):
        # Try direct JSON parse
        try:
            return json.loads(result_text).get("reason")
        except json.JSONDecodeError:
            pass
        # Extract from markdown JSON block
        import re
        match = re.search(r'"reason"\s*:\s*"([^"]+)"', result_text)
        if match:
            return match.group(1)
    return None


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
  python -m scripts.bench clear-cache
""",
    )
    parser.add_argument("command", choices=["index-speed", "search-quality", "search-value", "clear-cache"], help="Benchmark command")
    parser.add_argument("--verbose", "-v", action="store_true", help="Show detailed output")
    parser.add_argument("--question", "-q", help="Run specific question by ID")
    parser.add_argument("--export", "-e", metavar="FILE", help="Export results to JSON file (pytest-like format)")

    args = parser.parse_args()

    if args.command == "index-speed":
        index_speed.run(args.verbose)
    elif args.command == "search-quality":
        results = search_quality.run(args.question)
        if args.export:
            export_results(results, "search-quality", args.export)
    elif args.command == "search-value":
        results = search_value.run(args.question)
        if args.export:
            export_results(results, "search-value", args.export)
    elif args.command == "clear-cache":
        if RESPONSE_CACHE_DIR.exists():
            shutil.rmtree(RESPONSE_CACHE_DIR)
            log_success(f"Cleared response cache: {RESPONSE_CACHE_DIR}")
        else:
            log("Response cache already empty")


if __name__ == "__main__":
    main()
