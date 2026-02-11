"""A/B benchmark framework using claude CLI."""

import asyncio
import json
import os
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

from .common import (
    CYAN,
    NC,
    RunContext,
    _is_runtime_error,
    run_context,
    delete_cached_response,
    get_cache_key_from_cmd,
    get_cached_response,
    log,
    log_error,
    log_success,
    save_cached_response,
)

# Max parallel questions (each runs 2 Claude calls + 1 judge)
# Default to CPU count, fallback to 4
_default_parallel = os.cpu_count() or 4
MAX_PARALLEL_QUESTIONS = int(os.environ.get("CODEIX_BENCH_PARALLEL", _default_parallel))

# Max turns for test runs (some projects like zls need many turns)
MAX_TURNS_TEST = 25
# Max turns for judge (simple JSON output, needs only 1 turn)
MAX_TURNS_JUDGE = 5


# ─────────────────────────────────────────────────────────────────────────────
# Progress Display
# ─────────────────────────────────────────────────────────────────────────────

@dataclass
class QuestionProgress:
    """Progress state for a single question."""
    question_id: str
    max_turns_ab: int = MAX_TURNS_TEST
    max_turns_j: int = MAX_TURNS_JUDGE
    # Turn states: 'o' = output, 'x' = no output, '-' = unused, ' ' = pending
    turns_a: list[str] = field(default_factory=list)
    turns_b: list[str] = field(default_factory=list)
    turns_j: list[str] = field(default_factory=list)
    # Final result
    result: str = "..."  # "A", "B", "tie", "err", "..."
    # Done flags
    done_a: bool = False
    done_b: bool = False
    done_j: bool = False

    def format_turns(self, turns: list[str], max_turns: int, done: bool) -> str:
        """Format turn display with colors: [ooxxoo---------------]"""
        # Colors
        GREEN = "\033[32m"
        RED = "\033[31m"
        GRAY = "\033[90m"
        NC = "\033[0m"

        display = turns.copy()
        # Pad with spaces for pending turns
        while len(display) < max_turns:
            display.append(' ')
        # If done, replace trailing spaces with '-' (unused)
        if done:
            for i in range(len(display) - 1, -1, -1):
                if display[i] == ' ':
                    display[i] = '-'
                else:
                    break

        # Colorize each character
        colored = []
        for c in display[:max_turns]:
            if c == 'o':
                colored.append(f"{GREEN}o{NC}")
            elif c == 'x':
                colored.append(f"{RED}x{NC}")
            elif c == '-':
                colored.append(f"{GRAY}-{NC}")
            else:
                colored.append(c)  # '.' and ' ' stay as-is

        return f"[{''.join(colored)}]"

    def format_line(self) -> str:
        """Format full progress line."""
        RED = "\033[31m"
        NC = "\033[0m"

        a_str = self.format_turns(self.turns_a, self.max_turns_ab, self.done_a)
        b_str = self.format_turns(self.turns_b, self.max_turns_ab, self.done_b)
        j_str = self.format_turns(self.turns_j, self.max_turns_j, self.done_j)

        # Color labels red if session has errors (contains 'x')
        a_label = f"{RED}A{NC}" if 'x' in self.turns_a else "A"
        b_label = f"{RED}B{NC}" if 'x' in self.turns_b else "B"
        j_label = f"{RED}J{NC}" if 'x' in self.turns_j else "J"

        # Only show result when done (not "...")
        result_str = f" : {self.result}" if self.result != "..." else ""
        return f"[bench] {self.question_id:<28} {a_label} {a_str} {b_label} {b_str} {j_label} {j_str}{result_str}"


class ProgressDisplay:
    """Manages terminal progress display with ANSI escape codes."""

    def __init__(self, questions: list[dict]):
        self.questions = {q['id']: QuestionProgress(question_id=q['id']) for q in questions}
        self.question_order = [q['id'] for q in questions]
        self.lock = asyncio.Lock()
        self._lines_printed = 0
        self._enabled = sys.stderr.isatty()
        # Draw initial state
        self._redraw()

    async def update(self, question_id: str, **kwargs):
        """Update progress for a question and redraw."""
        async with self.lock:
            if question_id not in self.questions:
                return
            prog = self.questions[question_id]
            for key, value in kwargs.items():
                if hasattr(prog, key):
                    setattr(prog, key, value)
            self._redraw()

    async def add_turn(self, question_id: str, session: str, char: str = 'o'):
        """Add a turn to A, B, or J session.

        Args:
            char: Turn character:
                'o' = tool returned useful data
                '.' = no tool or tool returned empty/useless output
                'x' = tool error
                ' ' = no tool call (pending/text-only turn)
        """
        async with self.lock:
            if question_id not in self.questions:
                return
            prog = self.questions[question_id]
            if session == 'a':
                prog.turns_a.append(char)
            elif session == 'b':
                prog.turns_b.append(char)
            elif session == 'j':
                prog.turns_j.append(char)
            self._redraw()

    async def mark_done(self, question_id: str, session: str):
        """Mark a session as done."""
        async with self.lock:
            if question_id not in self.questions:
                return
            prog = self.questions[question_id]
            if session == 'a':
                prog.done_a = True
            elif session == 'b':
                prog.done_b = True
            elif session == 'j':
                prog.done_j = True
            self._redraw()

    async def set_result(self, question_id: str, result: str):
        """Set final result for a question."""
        async with self.lock:
            if question_id not in self.questions:
                return
            self.questions[question_id].result = result
            self._redraw()

    def _redraw(self):
        """Redraw all progress lines."""
        if not self._enabled:
            return
        # Move cursor up to beginning of our output
        if self._lines_printed > 0:
            sys.stderr.write(f"\033[{self._lines_printed}A")
        # Print all lines
        for qid in self.question_order:
            prog = self.questions[qid]
            line = prog.format_line()
            # Clear line and print
            sys.stderr.write(f"\033[2K{line}\n")
        sys.stderr.flush()
        self._lines_printed = len(self.question_order)

    def log(self, msg: str):
        """Print a log message above the progress display."""
        if not self._enabled:
            sys.stderr.write(f"{msg}\n")
            return
        # Move up, clear, print message, then redraw progress
        if self._lines_printed > 0:
            sys.stderr.write(f"\033[{self._lines_printed}A")
        sys.stderr.write(f"\033[2K{msg}\n")
        # Print blank lines to push progress down
        for _ in range(self._lines_printed - 1):
            sys.stderr.write("\033[2K\n")
        sys.stderr.flush()
        self._lines_printed = 0
        self._redraw()

    def finish(self):
        """Finalize display (no more updates)."""
        if not self._enabled:
            # Print final state for non-TTY
            for qid in self.question_order:
                prog = self.questions[qid]
                sys.stderr.write(f"{prog.format_line()}\n")
            sys.stderr.flush()


def build_codeix_cmd(
    mcp_config: str,
    prompt: str,
) -> list[str]:
    """Build a claude command with codeix MCP tools.

    Standardized arg order ensures cache key consistency across benchmarks.
    Uses stream-json for per-turn token/tool tracking.
    Uses --strict-mcp-config to ignore global MCP config.
    """
    return [
        "claude", "--print", "--output-format", "stream-json", "--verbose",
        "--no-session-persistence",
        "--max-turns", str(MAX_TURNS_TEST),
        "--allowedTools", "mcp__codeindex__*",
        "--strict-mcp-config",
        "--mcp-config", mcp_config,
        "-p", prompt,
    ]


def build_claude_cmd(
    prompt: str,
) -> list[str]:
    """Build a raw claude command (no MCP tools).

    Uses stream-json for per-turn token/tool tracking.
    Uses --disallowedTools to block all MCP tools while keeping built-in tools.
    """
    return [
        "claude", "--print", "--output-format", "stream-json", "--verbose",
        "--no-session-persistence",
        "--max-turns", str(MAX_TURNS_TEST),
        "--disallowedTools", "mcp__*",
        "-p", prompt,
    ]


def build_mcp_config(bin_path: str) -> str:
    """Build MCP config JSON for codeix.

    This makes the command line consistent across runs (for caching).
    """
    return json.dumps({
        "mcpServers": {
            "codeindex": {
                "command": bin_path,
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


def parse_stream_json(stdout_str: str) -> dict:
    """Parse stream-json output from claude CLI.

    Returns a dict with:
    - result: final response text
    - turns: list of {tools: [...], usage: {input_tokens, output_tokens}}
    - usage: total {input_tokens, output_tokens}
    - cost: total cost
    - Plus all other fields from the final result event
    """
    lines = stdout_str.strip().split("\n")
    turns = []
    final_result = {}
    # Track pending tool calls by ID to match with results
    pending_tools: dict[str, dict] = {}

    for line in lines:
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue

        event_type = event.get("type")

        if event_type == "assistant":
            # Extract tool usage from this turn
            message = event.get("message", {})
            content = message.get("content", [])
            tools_used = []
            for item in content:
                if item.get("type") == "tool_use":
                    tool_id = item.get("id")
                    tool_info = {
                        "id": tool_id,
                        "name": item.get("name"),
                        "input": item.get("input"),
                        "output": None,  # Will be filled from tool_result
                        "is_error": False,
                    }
                    tools_used.append(tool_info)
                    if tool_id:
                        pending_tools[tool_id] = tool_info
            # Usage is in message.usage, not directly in event
            usage = message.get("usage", {})
            turns.append({
                "tools": tools_used,
                "usage": {
                    "input_tokens": usage.get("input_tokens", 0),
                    "output_tokens": usage.get("output_tokens", 0),
                    "cache_read_input_tokens": usage.get("cache_read_input_tokens", 0),
                    "cache_creation_input_tokens": usage.get("cache_creation_input_tokens", 0),
                },
            })

        elif event_type == "user":
            # Tool results come in user messages
            message = event.get("message", {})
            content = message.get("content", [])
            for item in content:
                if item.get("type") == "tool_result":
                    tool_id = item.get("tool_use_id")
                    if tool_id and tool_id in pending_tools:
                        # Get output from tool_use_result if available, else from content
                        tool_result = event.get("tool_use_result", {})
                        output = tool_result.get("stdout") if isinstance(tool_result, dict) else None
                        if not output:
                            # content can be a string or a list of content blocks
                            item_content = item.get("content", "")
                            if isinstance(item_content, list):
                                # Extract text from content blocks
                                output = "\n".join(
                                    block.get("text", "") for block in item_content
                                    if isinstance(block, dict) and block.get("type") == "text"
                                )
                            else:
                                output = item_content
                        # Truncate large outputs
                        if isinstance(output, str) and len(output) > 2000:
                            output = output[:2000] + "... (truncated)"
                        pending_tools[tool_id]["output"] = output
                        pending_tools[tool_id]["is_error"] = item.get("is_error", False)

        elif event_type == "result":
            # Final result event - contains aggregated data
            usage = event.get("usage", {})
            final_result = {
                "result": event.get("result", ""),
                "subtype": event.get("subtype", ""),
                "is_error": event.get("is_error", False),
                "total_cost_usd": event.get("total_cost_usd") or event.get("cost_usd") or event.get("cost"),
                "usage": {
                    "input_tokens": usage.get("input_tokens", 0),
                    "output_tokens": usage.get("output_tokens", 0),
                    "cache_read_input_tokens": usage.get("cache_read_input_tokens", 0),
                    "cache_creation_input_tokens": usage.get("cache_creation_input_tokens", 0),
                },
                "session_id": event.get("session_id"),
                "num_turns": event.get("num_turns"),
            }

    # Combine everything
    result = final_result.copy()
    result["turns"] = turns

    # Calculate tool usage summary
    all_tools = []
    total_input = 0
    total_output = 0
    for turn in turns:
        all_tools.extend([t["name"] for t in turn["tools"]])
        total_input += turn["usage"]["input_tokens"]
        total_output += turn["usage"]["output_tokens"]

    result["tool_usage"] = {
        "tools_called": all_tools,
        "tool_count": len(all_tools),
        "unique_tools": list(set(all_tools)),
        "turns_with_tools": sum(1 for t in turns if t["tools"]),
    }
    # Use per-turn totals if final usage not available
    if not result.get("usage") or not result["usage"].get("input_tokens"):
        result["usage"] = {"input_tokens": total_input, "output_tokens": total_output}

    return result


def classify_tool_output(output: str | None, is_error: bool) -> str:
    """Classify tool output for progress display.

    Returns:
        'x' = tool error (is_error=True)
        'o' = tool returned useful data
        '.' = tool returned empty/useless output
    """
    if is_error:
        return 'x'

    if not output:
        return '.'

    # Check for useless outputs
    output_stripped = output.strip()

    # Empty
    if not output_stripped:
        return '.'

    # Empty array/object
    if output_stripped in ('[]', '{}', '""', "''"):
        return '.'

    # Common "nothing found" messages
    useless_patterns = [
        'No files found',
        'No matches found',
        'No results',
        'not found',
        '0 matches',
        '0 results',
    ]
    for pattern in useless_patterns:
        if pattern.lower() in output_stripped.lower():
            return '.'

    return 'o'


async def run_subprocess_streaming(
    cmd: list[str],
    cwd: Path | None = None,
    bin_dir: Path | None = None,
    on_turn: Callable[[str], None] | None = None,
) -> dict:
    """Run subprocess with streaming output, calling on_turn for each turn.

    Args:
        cmd: Command to run
        cwd: Working directory
        bin_dir: Directory to prepend to PATH
        on_turn: Callback called with turn char ('o', '.', 'x', ' ') for each turn
    """
    env = os.environ.copy()
    if bin_dir:
        env["PATH"] = f"{bin_dir}:{env.get('PATH', '')}"

    try:
        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            cwd=str(cwd) if cwd else None,
            env=env,
            start_new_session=True,
        )

        # Read stdout line by line for streaming
        lines = []
        # Track pending tool calls by ID for this turn
        pending_tools: dict[str, dict] = {}
        # Track if current turn has tool calls
        current_turn_tools: list[str] = []  # list of tool IDs

        while True:
            line = await proc.stdout.readline()
            if not line:
                break
            line_str = line.decode()
            lines.append(line_str)

            # Parse for progress updates
            if on_turn:
                try:
                    event = json.loads(line_str)
                    event_type = event.get("type")

                    if event_type == "assistant":
                        # New assistant turn - check for tool calls
                        message = event.get("message", {})
                        content = message.get("content", [])
                        current_turn_tools = []
                        for item in content:
                            if item.get("type") == "tool_use":
                                tool_id = item.get("id")
                                if tool_id:
                                    current_turn_tools.append(tool_id)
                                    pending_tools[tool_id] = {"output": None, "is_error": False}

                        # If no tool calls, emit ' ' for this turn
                        if not current_turn_tools:
                            on_turn(' ')

                    elif event_type == "user":
                        # Tool results come in user messages
                        message = event.get("message", {})
                        content = message.get("content", [])
                        for item in content:
                            if item.get("type") == "tool_result":
                                tool_id = item.get("tool_use_id")
                                if tool_id and tool_id in pending_tools:
                                    # Get output from tool_use_result if available
                                    tool_result = event.get("tool_use_result", {})
                                    output = tool_result.get("stdout") if isinstance(tool_result, dict) else None
                                    if not output:
                                        item_content = item.get("content", "")
                                        if isinstance(item_content, list):
                                            output = "\n".join(
                                                block.get("text", "") for block in item_content
                                                if isinstance(block, dict) and block.get("type") == "text"
                                            )
                                        else:
                                            output = item_content
                                    pending_tools[tool_id]["output"] = output
                                    pending_tools[tool_id]["is_error"] = item.get("is_error", False)

                        # If we have all results for current turn's tools, emit char
                        if current_turn_tools:
                            all_done = all(
                                tool_id in pending_tools and pending_tools[tool_id].get("output") is not None
                                for tool_id in current_turn_tools
                            )
                            # Also emit if we got tool_result for any pending tool
                            got_results = any(
                                tool_id in pending_tools and pending_tools[tool_id].get("output") is not None
                                for tool_id in current_turn_tools
                            )
                            if got_results:
                                # Classify based on best tool result
                                # If any tool succeeded with data, use 'o'
                                # If any tool errored, use 'x'
                                # Otherwise use '.'
                                chars = []
                                for tool_id in current_turn_tools:
                                    if tool_id in pending_tools:
                                        t = pending_tools[tool_id]
                                        chars.append(classify_tool_output(t.get("output"), t.get("is_error", False)))

                                # Priority: 'x' (error) > 'o' (success) > '.' (empty)
                                if 'x' in chars:
                                    on_turn('x')
                                elif 'o' in chars:
                                    on_turn('o')
                                else:
                                    on_turn('.')
                                current_turn_tools = []  # Reset for next turn

                except json.JSONDecodeError:
                    pass

        # Wait for process to complete and get stderr
        _, stderr = await proc.communicate()
        stderr_str = stderr.decode() if stderr else ""

        stdout_str = "".join(lines)
        if not stdout_str:
            return {"result": "", "error": stderr_str}

        # Parse the collected output
        if "\n" in stdout_str.strip():
            result = parse_stream_json(stdout_str)
            if stderr_str:
                result["error"] = stderr_str
            return result
        else:
            try:
                return json.loads(stdout_str)
            except json.JSONDecodeError:
                return {"result": stdout_str, "error": stderr_str}
    except asyncio.CancelledError:
        if 'proc' in locals():
            proc.terminate()
        raise
    except Exception as e:
        return {"result": "", "error": str(e)}


async def run_subprocess(cmd: list[str], cwd: Path | None = None, bin_dir: Path | None = None) -> dict:
    """Run a subprocess command without TTY (prevents terminal issues with claude).

    If bin_dir is provided, it's prepended to PATH so versioned binaries can be found.
    Parses stream-json output to extract per-turn tool/token data.
    """
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

        if not stdout_str:
            return {"result": "", "error": stderr_str}

        # Check if this is stream-json (multiple lines) or regular json
        if "\n" in stdout_str.strip():
            # Stream JSON - parse it
            result = parse_stream_json(stdout_str)
            if stderr_str:
                result["error"] = stderr_str
            return result
        else:
            # Regular JSON (e.g., judge calls still use --output-format json)
            try:
                return json.loads(stdout_str)
            except json.JSONDecodeError:
                return {"result": stdout_str, "error": stderr_str}
    except asyncio.CancelledError:
        if 'proc' in locals():
            proc.terminate()
        raise
    except Exception as e:
        return {"result": "", "error": str(e)}


async def run_question(
    q: dict,
    config: ABConfig,
    ctx: RunContext,
    progress: ProgressDisplay | None = None,
) -> dict | None:
    """Run A/B test for a single question.

    Cache key = command line. This works because:
    - Binary is named codeix-{version} (version embedded in filename)
    - cwd is the project repo (so paths are relative)
    - All params (--max-turns, prompt, etc.) are in the command
    """
    qid = q['id']

    # Get commands first (needed for cache keys)
    # This is cheap - just builds the command strings
    cmd_a, cwd_a, cmd_b, cwd_b = config.get_commands(q, ctx)

    # Cache key = command line
    cache_key_a = get_cache_key_from_cmd(cmd_a)
    cache_key_b = get_cache_key_from_cmd(cmd_b)

    cached_a = get_cached_response(cache_key_a)
    cached_b = get_cached_response(cache_key_b)

    # Helper to classify a turn from cached response
    def classify_cached_turn(turn: dict) -> str:
        """Classify a cached turn for progress display."""
        tools = turn.get("tools", [])
        if not tools:
            return ' '  # No tool calls

        # Check each tool's output
        chars = []
        for tool in tools:
            output = tool.get("output")
            is_error = tool.get("is_error", False)
            chars.append(classify_tool_output(output, is_error))

        # Priority: 'x' (error) > 'o' (success) > '.' (empty)
        if 'x' in chars:
            return 'x'
        elif 'o' in chars:
            return 'o'
        else:
            return '.'

    # Update progress for cached responses
    if progress:
        if cached_a:
            for t in cached_a["response"].get("turns", []):
                await progress.add_turn(qid, 'a', classify_cached_turn(t))
            await progress.mark_done(qid, 'a')
        if cached_b:
            for t in cached_b["response"].get("turns", []):
                await progress.add_turn(qid, 'b', classify_cached_turn(t))
            await progress.mark_done(qid, 'b')

    # If both cached, skip setup entirely
    if cached_a and cached_b:
        response_a = cached_a["response"]
        response_b = cached_b["response"]
    else:
        # Only setup (clone + build) if we need to run
        if not cached_a and config.setup_a and not config.setup_a(q, ctx):
            log_error(f"  Setup A failed for {q['id']}")
            if progress:
                await progress.set_result(qid, "err")
            return None
        if not cached_b and config.setup_b and not config.setup_b(q, ctx):
            log_error(f"  Setup B failed for {q['id']}")
            if progress:
                await progress.set_result(qid, "err")
            return None

        # Run A then B sequentially (questions are already parallel)
        if not cached_a:
            # Streaming callback for real-time progress
            def on_turn_a(char: str):
                asyncio.create_task(progress.add_turn(qid, 'a', char)) if progress else None

            response_a = await run_subprocess_streaming(cmd_a, cwd_a, ctx.bin_dir, on_turn_a if progress else None)
            # Fallback to non-streaming on runtime error (Claude CLI streaming buffer issue)
            if _is_runtime_error(response_a):
                if progress:
                    prog = progress.questions.get(qid)
                    if prog:
                        prog.turns_a = []
                # Build non-streaming command (replace stream-json with json, remove --verbose)
                cmd_a_fallback = [c for c in cmd_a if c != "--verbose"]
                cmd_a_fallback = [c if c != "stream-json" else "json" for c in cmd_a_fallback]
                response_a = await run_subprocess(cmd_a_fallback, cwd_a, ctx.bin_dir)
            if progress:
                await progress.mark_done(qid, 'a')
        else:
            response_a = cached_a["response"]

        if not cached_b:
            def on_turn_b(char: str):
                asyncio.create_task(progress.add_turn(qid, 'b', char)) if progress else None

            response_b = await run_subprocess_streaming(cmd_b, cwd_b, ctx.bin_dir, on_turn_b if progress else None)
            # Fallback to non-streaming on runtime error (Claude CLI streaming buffer issue)
            if _is_runtime_error(response_b):
                if progress:
                    prog = progress.questions.get(qid)
                    if prog:
                        prog.turns_b = []
                # Build non-streaming command (replace stream-json with json, remove --verbose)
                cmd_b_fallback = [c for c in cmd_b if c != "--verbose"]
                cmd_b_fallback = [c if c != "stream-json" else "json" for c in cmd_b_fallback]
                response_b = await run_subprocess(cmd_b_fallback, cwd_b, ctx.bin_dir)
            if progress:
                await progress.mark_done(qid, 'b')
        else:
            response_b = cached_b["response"]

        # Save to cache
        if not cached_a:
            save_cached_response(cache_key_a, response_a, {"question_id": q["id"], "label": config.label_a, "cmd": cmd_a})
        if not cached_b:
            save_cached_response(cache_key_b, response_b, {"question_id": q["id"], "label": config.label_b, "cmd": cmd_b})

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

    # Skip judging if either response has an error
    if error_a or error_b:
        if progress:
            await progress.set_result(qid, "err")
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
        "--max-turns", str(MAX_TURNS_JUDGE),
        "--disallowedTools", "mcp__*",
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
        if progress:
            await progress.add_turn(qid, 'j', 'o')
            await progress.mark_done(qid, 'j')
            # Set result immediately for cached judge
            winner = parse_judge_winner(judge_response)
            await progress.set_result(qid, winner)
    else:
        # Run judge via subprocess (no tools needed, no streaming needed)
        judge_response = await run_subprocess(judge_cmd)
        if progress:
            await progress.add_turn(qid, 'j', 'o')  # Judge always produces output
            await progress.mark_done(qid, 'j')
            # Set result immediately
            winner = parse_judge_winner(judge_response)
            await progress.set_result(qid, winner)
        # With --json-schema, output is in structured_output field
        structured = judge_response.get("structured_output", {})
        if structured and "winner" in structured:
            save_cached_response(judge_cache_key, judge_response, {"question_id": q["id"], "type": "judge", "cmd": judge_cmd})
        cached_judge_flag = False

    result = {
        "question": q,
        "response_a": response_a,
        "response_b": response_b,
        "judge": judge_response,
        "cost_a": response_a.get("total_cost_usd"),
        "cost_b": response_b.get("total_cost_usd"),
        "usage_a": response_a.get("usage", {}),
        "usage_b": response_b.get("usage", {}),
        "tool_usage_a": response_a.get("tool_usage", {}),
        "tool_usage_b": response_b.get("tool_usage", {}),
        "turns_a": response_a.get("turns", []),
        "turns_b": response_b.get("turns", []),
        "cached_a": cached_a is not None,
        "cached_b": cached_b is not None,
        "cached_judge": cached_judge_flag,
        "error_a": None,  # No errors if we got here (errors returned early above)
        "error_b": None,
    }

    result_file = ctx.results_dir / f"{q['id']}.json"
    result_file.write_text(json.dumps(result, indent=2))

    # Update progress with final result
    if progress:
        winner = parse_judge_winner(judge_response)
        await progress.set_result(qid, winner)

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

    with run_context() as ctx:
        log(f"Running {config.name} with {len(questions)} question(s) in parallel")
        log(f"Run dir: {ctx.run_dir}")
        print()

        # Setup binaries (once, before running questions)
        # Binaries are named with version embedded (e.g., codeix-abc123)
        bin_a, bin_b = config.setup_run(ctx)
        log(f"A: {config.label_a} ({bin_a})")
        log(f"B: {config.label_b} ({bin_b})")
        print()

        # Create progress display
        progress = ProgressDisplay(questions)

        # Create semaphore to limit parallelism
        sem = asyncio.Semaphore(MAX_PARALLEL_QUESTIONS)

        async def run_with_sem(q: dict) -> dict | None:
            async with sem:
                return await run_question(q, config, ctx, progress)

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

        # Finalize progress display
        progress.finish()

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
