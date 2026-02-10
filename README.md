# codeix

**[codeix.dev](https://codeix.dev)** · Portable, composable code index. Your AI agent finds the right function on the first try — no scanning, no guessing, no wasted tokens.

```
codeix                 # start MCP server, watch for changes
codeix build           # parse source files, write .codeindex
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

- **Committed to git** — the index is a `.codeindex` directory you commit with your code. Clone the repo, the index is already there. No re-indexing.
- **Shareable** — library authors can ship `.codeindex` in their npm/PyPI/crates.io package. Consumers get instant navigation of dependencies.
- **Composable** — the MCP server auto-discovers dependency indexes and mounts them. Query your code and your dependencies in one place.
- **Structured for LLMs** — symbols have kinds, signatures, parent relationships, and line ranges. The agent gets exactly what it needs in one tool call instead of piecing it together from raw text.
- **Prose search** — `search --scope text` targets comments, docstrings, and string literals specifically. Find TODOs, find the error message a user reported, find what a function's docstring says — without noise from code.
- **Fast** — builds in seconds, queries in milliseconds. Rust + tree-sitter + in-memory SQLite FTS5 under the hood.

## The `.codeindex` format

An open, portable format for structured code indexing. Plain JSONL files you commit alongside your code — git-friendly diffs, human-readable with `grep` and `jq`, no binary blobs.

```
.codeindex/
  index.json        # manifest: version, name, languages
  files.jsonl       # one line per source file (path, lang, hash, line count)
  symbols.jsonl     # one line per symbol (functions, classes, imports, with signatures)
  texts.jsonl       # one line per comment, docstring, string literal
```

Any tool that can parse JSON can consume a `.codeindex`. Codeix builds it using [tree-sitter](https://tree-sitter.github.io/), and AI agents query it through [MCP](https://modelcontextprotocol.io/) (Model Context Protocol).

**Example** — `symbols.jsonl`:
```jsonl
{"file":"src/main.py","name":"os","kind":"import","line":[1,1]}
{"file":"src/main.py","name":"Config","kind":"class","line":[22,45]}
{"file":"src/main.py","name":"Config.__init__","kind":"method","line":[23,30],"parent":"Config","sig":"def __init__(self, path: str, debug: bool = False)"}
{"file":"src/main.py","name":"main","kind":"function","line":[48,60],"sig":"def main(args: list[str]) -> int"}
```

## Ship your index with your package

Include `.codeindex` in your package and every developer who depends on you gets instant navigation of your API — no setup, no re-indexing.

Works with Git repos, npm, PyPI, and crates.io.

## MCP tools

Eight tools, zero setup. The agent queries immediately — no init, no config, no refresh.

| Tool | What it does |
|---|---|
| `explore` | Explore project structure: metadata, subprojects, files grouped by directory |
| `search` | Unified full-text search across symbols, files, and texts (FTS5, BM25-ranked) with scope/kind/path/project filters |
| `get_file_symbols` | List all symbols in a file |
| `get_children` | Get children of a class/module |
| `get_imports` | List imports for a file |
| `get_callers` | Find all places that call or reference a symbol |
| `get_callees` | Find all symbols that a function/method calls |
| `flush_index` | Flush pending index changes to disk |

## Project discovery

Launch `codeix` from any directory. It walks downward and treats every directory containing `.git/` as a separate project — each gets its own `.codeindex`.

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
| Markdown | `lang-markdown` | yes | `.md` `.markdown` |

### Markdown support

Markdown files are parsed for **headings** (both ATX `#` and Setext underline styles) which are indexed as `section` symbols with hierarchical parent-child relationships — enabling TOC extraction and document structure navigation.

Fenced code blocks are extracted as `code` text entries, parented to their containing section.

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
- **Minimal surface** — 7 query tools, zero management plumbing

## Architecture

See [`docs/architecture.md`](docs/architecture.md) for the full set of architecture decision records.

## License

MIT OR Apache-2.0
