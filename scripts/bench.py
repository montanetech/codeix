#!/usr/bin/env python3
"""
Codeix Benchmark Suite

Usage:
    python scripts/bench.py index-speed [--verbose]
    python scripts/bench.py search-quality [--question ID]
    python scripts/bench.py search-value [--question ID]

Commands:
    index-speed     Quantitative indexing speed benchmark
    search-quality  A/B: prod codeix vs dev codeix
    search-value    A/B: codeix vs raw Claude (no MCP)
"""

from bench import index_speed, search_quality, search_value
from bench.__main__ import main

if __name__ == "__main__":
    main()
