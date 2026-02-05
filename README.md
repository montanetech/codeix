# codeix

Portable, composable code index. Build once with tree-sitter, query anywhere via MCP.

```
codeix                 # start MCP server, watch for changes
codeix build           # parse source files, write .codeindex/
codeix serve --no-watch  # serve without file watching
```

## Why

AI coding agents spend most of their token budget *finding* code before they can *work on* it. They grep, read files, grep again, backtrack. On a large codebase the agent might burn thousands of tokens just locating the right function — or worse, miss it entirely and hallucinate.

**Codeix gives the agent a pre-built map of your codebase.** One structured query returns the symbol name, file, line range, signature, and parent — no scanning, no guessing.

### What existing tools get wrong

| Problem | What happens today |
|---|---|
| **No structure** | `grep` finds text matches, not symbols. The agent can't distinguish a function definition from a comment mentioning it. |
| **Slow re-parsing** | Python-based indexers re-parse everything on startup. On large codebases, you wait. |
| **Not shareable** | Indexes are local caches — ephemeral, per-machine. A new developer or CI runner starts from scratch. |
| **No composition** | Monorepo with 10 packages? Dependencies with useful APIs? No way to query across boundaries. |
| **Prose is invisible** | TODOs, docstrings, error messages — searchable by grep but not *selectively*. You can't search only comments without also matching code. |

### What codeix does differently

- **Committed to git** — the index is a `.codeindex/` directory you commit with your code. Clone the repo, the index is already there. No re-indexing.
- **Shareable** — library authors can ship `.codeindex/` in their npm/PyPI/crates.io package. Consumers get instant navigation of dependencies.
- **Composable** — the MCP server auto-discovers dependency indexes and mounts them. Query your code and your dependencies in one place.
- **Structured for LLMs** — symbols have kinds, signatures, parent relationships, and line ranges. The agent gets exactly what it needs in one tool call instead of piecing it together from raw text.
- **Prose search** — `search_texts` targets comments, docstrings, and string literals specifically. Find TODOs, find the error message a user reported, find what a function's docstring says — without noise from code.
- **Fast** — Rust + tree-sitter + in-memory SQLite FTS5. Builds in seconds, queries in milliseconds.

## What it does

Codeix scans your source code with [tree-sitter](https://tree-sitter.github.io/), extracts symbols, imports, comments, and docstrings, then writes a `.codeindex/` directory you commit alongside your code.

AI agents query it through [MCP](https://modelcontextprotocol.io/) (Model Context Protocol) to navigate codebases without re-parsing anything.

## The index format

```
.codeindex/
  index.json        # manifest: version, name, languages
  files.jsonl       # one line per source file (path, lang, hash, line count)
  symbols.jsonl     # one line per symbol (functions, classes, imports, with signatures)
  texts.jsonl       # one line per comment, docstring, string literal
```

Plain JSONL. Git-friendly diffs. Human-readable with `grep` and `jq`. No binary blobs.

**Example** — `symbols.jsonl`:
```jsonl
{"file":"src/main.py","name":"os","kind":"import","line":[1,1]}
{"file":"src/main.py","name":"Config","kind":"class","line":[22,45]}
{"file":"src/main.py","name":"Config.__init__","kind":"method","line":[23,30],"parent":"Config","sig":"def __init__(self, path: str, debug: bool = False)"}
{"file":"src/main.py","name":"main","kind":"function","line":[48,60],"sig":"def main(args: list[str]) -> int"}
```

## MCP tools

Six tools, zero setup. The agent queries immediately — no init, no config, no refresh.

| Tool | What it does |
|---|---|
| `search_symbols` | Fuzzy search across all symbols (FTS5, BM25-ranked) |
| `search_files` | Find files by name, path, or language |
| `search_texts` | Full-text search on comments, docstrings, strings |
| `get_file_symbols` | List all symbols in a file |
| `get_symbol_children` | Get children of a class/module |
| `get_imports` | List imports for a file |

## Project discovery

Launch `codeix` from any directory. It walks downward and treats every directory containing `.git/` as a separate project — each gets its own `.codeindex/`.

Works uniformly for single repos, monorepos, sibling repos, and git submodules. No config needed.

## Languages

Tree-sitter grammars, feature-gated at compile time:

| Language | Feature flag | Default | Extensions |
|---|---|---|---|
| Python | `lang-python` | yes | `.py` `.pyi` `.pyw` |
| Rust | `lang-rust` | yes | `.rs` |
| JavaScript | `lang-javascript` | yes | `.js` `.mjs` `.cjs` `.jsx` |
| TypeScript | `lang-typescript` | yes | `.ts` `.mts` `.cts` `.tsx` |
| Go | `lang-go` | yes | `.go` |
| Java | `lang-java` | yes | `.java` |
| C | `lang-c` | yes | `.c` `.h` |
| C++ | `lang-cpp` | yes | `.cpp` `.cc` `.cxx` `.hpp` `.hxx` |
| Ruby | `lang-ruby` | yes | `.rb` `.rake` `.gemspec` |
| C# | `lang-csharp` | yes | `.cs` |

### Embedded scripts

HTML, Vue, Svelte, and Astro files are preprocessed to extract embedded `<script>` blocks, which are then parsed with the JavaScript or TypeScript grammar:

| Format | Extensions | Script detection |
|---|---|---|
| HTML | `.html` `.htm` | `<script>` tags, with optional `lang="ts"` |
| Vue | `.vue` | `<script>` and `<script setup>`, with optional `lang="ts"` |
| Svelte | `.svelte` | `<script>`, with optional `lang="ts"` |
| Astro | `.astro` | `---` frontmatter (always TypeScript) + optional `<script>` tags |

Line numbers in the index point to the original file, not the extracted script block.

## Install

```sh
# npm / npx — run without installing
npx codeix

# pip / uvx — run without installing
uvx codeix

# Rust
cargo install codeix

# Homebrew
brew install codeix

# Or build from source
git clone https://github.com/montanetech/codeix.git
cd codeix
cargo build --release
```

All channels install the same single binary. No runtime dependencies.

## Usage

```sh
# Build the index for the current project
codeix build

# Build from a specific directory (discovers all git repos below)
codeix build ~/projects

# Start MCP server (default command, watches for changes)
codeix

# Or explicitly
codeix serve
codeix serve --no-watch
```

### MCP client configuration

Add to your MCP client config (e.g. Claude Desktop, Cursor):

```json
{
  "mcpServers": {
    "codeix": {
      "command": "codeix"
    }
  }
}
```

## Design principles

- **Local only** — no network, no API keys, works offline and air-gapped
- **Deterministic** — same source always produces the same index (clean diffs)
- **Composable** — dependency indexes are auto-discovered and mounted at query time
- **Minimal surface** — 6 query tools, zero management plumbing

## Architecture

See [`docs/architecture.md`](docs/architecture.md) for the full set of architecture decision records.

## License

MIT OR Apache-2.0
