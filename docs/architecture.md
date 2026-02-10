# Codeindex — Architecture Decisions

## Problem Statement

Existing code indexing tools (e.g. code-index-mcp) have three key limitations:

1. **Performance** — Python-based indexers are slow for large codebases
2. **No sharing** — indexes are local caches, not shareable artifacts
3. **No composition** — no way to aggregate indexes across monorepo packages or dependencies

## Vision

A **portable, composable code index format** with a CLI to build it and an MCP server to query it.

A developer builds the index, commits it to the repo. Consumers (humans or AI agents) use it instantly — no re-indexing.

---

## ADR-001: Index format is text-based JSONL

**Context:** The index must be cross-platform, git-friendly (diffable, mergeable), and easy to inspect.

**Decision:** The index is a set of text files using JSONL (JSON Lines) format.

**Consequences:**
- Diffs are clean — one line per entry, changes cluster by file path
- No binary portability issues (endianness, platform-specific)
- Slightly larger than binary formats, but manageable at scale
- Human-readable, inspectable with standard tools (grep, jq)

**Rejected alternatives:**
- SQLite: binary, opaque diffs, merge conflicts in git
- FlatBuffers/MessagePack: binary, not git-friendly
- One JSON file per source file: too many files, noisy in git

---

## ADR-002: Index structure — 4 files

**Decision:** A `.codeindex/` directory at the project root with:

```
.codeindex/
  index.json        # manifest: version, name, metadata
  files.jsonl       # one line per source file
  symbols.jsonl     # one line per symbol, sorted by file then line
  texts.jsonl       # one line per comment/string/docstring
```

### `index.json` — manifest
```json
{
  "version": "1.0",
  "name": "my-project",
  "root": ".",
  "languages": ["python", "typescript"]
}
```

Self-description only. No dependency wiring.

### `files.jsonl` — file registry
```jsonl
{"path":"src/main.py","lang":"python","hash":"a1b2c3","lines":142}
{"path":"src/utils/helpers.py","lang":"python","hash":"d4e5f6","lines":87}
```

Sorted by path. One line per source file.

### `symbols.jsonl` — symbol index (definitions + imports)
```jsonl
{"file":"src/main.py","name":"os","kind":"import","line":[1,1]}
{"file":"src/main.py","name":"utils.parse","kind":"import","line":[2,2],"alias":"parse"}
{"file":"src/main.py","name":"Config","kind":"class","line":[22,45]}
{"file":"src/main.py","name":"Config.__init__","kind":"method","line":[23,30],"parent":"Config","sig":"def __init__(self, path: str, debug: bool = False)"}
{"file":"src/main.py","name":"main","kind":"function","line":[48,60],"sig":"def main(args: list[str]) -> int"}
```

Sorted by file path, then line number. Flat structure with `parent` field for nesting.

Optional `sig` field on function/method/class symbols: the raw declaration text extracted from the AST (parameters + return type), as it appears in source. No normalization across languages — the LLM already understands each language's syntax. Omitted when not available (e.g. symbol kinds where signatures don't apply).

Imports are included as symbols with `kind: "import"`. This enables dependency graph queries ("what does this file use?", "who imports this module?") without a separate file. References (usage sites) are **not** included — they require semantic/type resolution that tree-sitter can't provide reliably.

### `texts.jsonl` — comments, docstrings, string literals
```jsonl
{"file":"src/main.py","kind":"docstring","line":[15,18],"text":"Validates user credentials against the database.","parent":"authenticate"}
{"file":"src/main.py","kind":"comment","line":[45,45],"text":"TODO: add rate limiting"}
{"file":"src/main.py","kind":"string","line":[22,22],"text":"Invalid credentials for user: %s"}
```

Sorted by file path, then line number. Extracted by tree-sitter (comment/string AST node types).

**What's included:** comments, docstrings, string literals above a minimum length.
**What's excluded:** trivial strings (`""`, `"\n"`), auto-generated boilerplate.
**Why:** enables FTS on human-written prose — find TODOs, error messages, documentation — which `rg` can't selectively target (it can't distinguish comments from code).

At serve time, all JSONL files are loaded into an in-memory SQLite database with FTS5 indexes. This provides a single query engine for all search: symbol lookup, file discovery, and full-text search on prose — with fuzzy matching and BM25 ranking for free.

**Scale estimate** (10k-file project):
- `files.jsonl`: ~10k lines, ~500KB
- `symbols.jsonl`: ~100k lines, ~5MB
- `texts.jsonl`: ~50k lines, ~3MB

---

## ADR-003: Index is self-contained, no dependency declarations

**Context:** We need composition for monorepos and dependencies. The question is where the wiring lives.

**Decision:** The `.codeindex/` directory describes only its own code. It never references other indexes.

Dependency resolution happens at **query time** by the CLI/MCP server, which:

1. Reads the project's package manifest (`package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod`, `composer.json`, etc.)
2. Resolves dependency locations on disk using standard package manager conventions
3. Looks for `.codeindex/` in each resolved dependency
4. Mounts found indexes automatically

**Consequences:**
- Index format stays simple and universal
- No coupling to any package manager in the index itself
- Package manager support is a pluggable concern in the CLI/server
- A library author can ship `.codeindex/` in their published package (npm, PyPI, crates.io)

**Examples:**

Monorepo with npm workspaces:
```
server reads package.json → workspaces: ["packages/*"]
  → finds packages/core/.codeindex/
  → finds packages/ui/.codeindex/
  → mounts both automatically
```

Python project:
```
server reads pyproject.toml → dependencies
  → checks .venv/lib/.../requests/.codeindex/
  → if present, loads it
```

---

## ADR-004: Parsing via tree-sitter

**Status:** Strong candidate — best available option as of 2025.

**Context:** We need reliable, fast, multi-language parsing to extract symbols (names, kinds, line ranges, parent relationships). This is shallow extraction, not full semantic analysis.

**Decision:** Use tree-sitter for AST parsing with language-specific `tags.scm` query files.

**Why tree-sitter:**
- Industry standard — used by GitHub, Aider, Zed, Neovim, Helix, ast-grep, Sourcegraph
- One unified C library + query API for all languages
- 100+ language grammars available, community-maintained
- Built-in error recovery (handles incomplete/broken code)
- Adding a language = adding one grammar crate + one extractor module

**Alternatives considered:**

| Tool | Verdict | Why not |
|---|---|---|
| Universal Ctags | Declining | Regex-based, less accurate, external binary |
| ANTLR | Overkill | No pre-built grammars ecosystem, heavier |
| LSP servers | Too heavy | Need N servers installed, slow startup |
| SCIP (Sourcegraph) | Too heavy | Per-language indexers, not incremental |
| Stack Graphs (GitHub) | Dead | Archived Sept 2025 — too complex to maintain |
| Per-language native parsers | Fragmented | N install requirements, N output formats |

**Limitations to accept:**
- Syntax only, no semantic understanding (no type resolution, no cross-file reference resolution)
- Grammar quality varies for niche languages — fallback to file-level indexing only
- Each language grammar is a C library — packaging/distribution concern (not architectural)

**Supported languages (10 + 3 SFC formats):**

| Language | Grammar crate | Feature flag |
|---|---|---|
| Python | `tree-sitter-python` | `lang-python` |
| Rust | `tree-sitter-rust` | `lang-rust` |
| JavaScript | `tree-sitter-javascript` | `lang-javascript` |
| TypeScript / TSX | `tree-sitter-typescript` | `lang-typescript` |
| Go | `tree-sitter-go` | `lang-go` |
| Java | `tree-sitter-java` | `lang-java` |
| C | `tree-sitter-c` | `lang-c` |
| C++ | `tree-sitter-cpp` | `lang-cpp` |
| Ruby | `tree-sitter-ruby` | `lang-ruby` |
| C# | `tree-sitter-c-sharp` | `lang-csharp` |

HTML files (`.html`, `.htm`) and Single File Components (Vue `.vue`, Svelte `.svelte`, Astro `.astro`) are preprocessed to extract `<script>` blocks (and Astro `---` frontmatter), which are then parsed with the JS/TS grammar. Line numbers are adjusted back to the original file.

**Consequences:**
- Consistent parsing across all supported languages
- Language support is additive — add a grammar crate + one extractor module per language
- Tree-sitter is C-based — fast regardless of host language
- We rely on community-maintained grammars — risk of stale/buggy grammars for rare languages

---

## ADR-005: Three commands — build, serve, watch

**Decision:** The tool has three commands:

### `codeindex build`
- Scans source files, parses with tree-sitter, writes `.codeindex/`
- Incremental: only re-indexes files whose hash changed
- Deterministic output: same source → same index (for clean diffs)
- Can be run manually, in CI, or from a git pre-commit hook

### `codeindex serve`
- Loads `.codeindex/` into memory (or in-memory SQLite for complex queries)
- Auto-discovers and mounts dependency indexes
- Exposes search/query tools via MCP protocol

### `codeindex serve --watch`
- Combines serve + file watching
- On source file change (debounced): re-indexes changed files, updates in-memory state, flushes `.codeindex/` to disk
- The index stays in sync with the code at all times
- `.codeindex/` shows up in `git status` like any other modified file — commit it when you commit your code

**Consequences:**
- Build and serve are decoupled — you can build on CI, serve locally
- The index is the contract between the two
- MCP server can use any in-memory representation for speed (not constrained by on-disk format)
- Watch mode makes the index feel like a lockfile — always there, always current, no extra step

---

## ADR-006: Deterministic, sorted output

**Context:** Since the index lives in git, diffs must be meaningful and minimal.

**Decision:** All JSONL output is:
- Sorted by a stable key (file path, then line number)
- Deterministic: same input always produces the same output
- No timestamps or random IDs in the index entries

**Consequences:**
- Rebuilding the index without source changes produces zero diff
- PR reviews can meaningfully show what changed in the index
- Merge conflicts are rare and resolvable (sorted order = locality)

---

## ADR-007: Index stays on disk via watch mode — committed like a lockfile

**Context:** The index lives in git. When should it be written to disk?

**Decision:** Multiple strategies, layered on top of one core primitive (`codeindex build`):

| Strategy | How | When |
|---|---|---|
| Manual | `codeindex build` | Before committing, like `npm run build` |
| Watch | `codeindex serve --watch` | Continuous — writes to disk on every source change (debounced) |
| Pre-commit hook | `codeindex build` in git hook | Automatically on `git commit` |
| CI | `codeindex build` in pipeline | On push/merge, commits the result |

The **recommended workflow** is watch mode:

```
$ codeindex serve --watch     # start once, forget about it

# ... edit code ...
# .codeindex/ updates automatically

$ git add -A                  # .codeindex/ is just another changed file
$ git commit -m "feat: ..."
```

**Consequences:**
- No extra step for developers — index is always current
- `.codeindex/` behaves like a lockfile (package-lock.json, Cargo.lock)
- All strategies produce the same output (deterministic build)
- Teams can choose the strategy that fits their workflow

**Rejected alternatives:**
- Only manual build: too easy to forget, index drifts out of sync
- Only CI: developers don't benefit locally during development
- Auto-commit: too invasive, pollutes git history

---

## ADR-008: Core is local only — no external services

**Context:** Many developer tools require API keys, cloud services, or network access. This creates friction, vendor lock-in, and breaks air-gapped environments.

**Decision:** Codeindex core requires **zero external services**. Everything runs locally, offline, with no API keys.

**Principles:**
- **Local only** — no network calls, no API keys, works offline and air-gapped
- **No mandatory heavy dependencies** — optional features can bring their own deps, but core stays lean
- **Deterministic core** — the `.codeindex/` on-disk format is deterministic and text-based

**Consequences:**
- Core stays simple, fast, and portable
- Works everywhere: CI, containers, air-gapped networks, developer laptops
- No vendor lock-in

---

## ADR-009: MCP tool surface — 8 tools, zero plumbing

**Context:** The MCP server exposes tools to AI agents. Competing servers (code-index-mcp: 13 tools, claude-context: 4 tools, Serena: 21 tools) mix search tools with management plumbing (init, refresh, configure watcher, temp directories). This forces agents to manage infrastructure before they can query.

**Decision:** 8 tools, split into discovery (explore), search (unified FTS), lookup (exact, structural), and graph (callers/callees). Zero management tools — the index is pre-built, the server loads it automatically.

### Discovery tool

| Tool | Input | Returns |
|---|---|---|
| `explore` | optional `path`, `project`, `max_entries` | Project metadata, subprojects, files grouped by directory |

**Parameters:**
- `path`: Scope exploration to a subdirectory (auto-resolves to subproject if matching)
- `project`: Explicit project filter (relative path from workspace root)
- `max_entries`: Budget for files to display (default: 200). If total exceeds budget, files are capped per directory with "+N files" indicators

### Unified search tool (FTS5, BM25-ranked)

| Tool | Input | Returns |
|---|---|---|
| `search` | `query`, optional `scope`/`kind`/`path`/`project` filters, pagination | Matching symbols, files, and/or texts with relevance ranking and code snippets |

**Parameters:**
- `query` (required): FTS5 search terms — supports `"foo bar"` (AND), `"foo OR bar"`, `"foo*"` (prefix), `"foo -bar"` (exclude)
- `scope`: Filter by type — array of `"symbol"`, `"file"`, `"text"`. Default: all three
- `kind`: Filter by kind (see table below)
- `path`: Glob pattern for file paths — `"src/**/*.rs"`, `"**/test_*.py"`
- `project`: Limit to a specific indexed project (relative path from workspace root)
- `limit`/`offset`: Pagination (default limit: 10)
- `snippet_lines`: Code context lines per result (default: 10, use 0 for none, -1 for full)

**Symbol kinds by language:**

| Kind | Languages | Notes |
|------|-----------|-------|
| `function` | All | Top-level functions |
| `method` | All | Functions inside class/struct/impl |
| `class` | Python, Ruby, JS/TS, Java, C#, C++ | Class declarations |
| `struct` | C, C++, Go, Rust, C#, Java | **Go/Rust/C use `struct`, not `class`** |
| `interface` | Go, Java, C#, TypeScript | **Rust uses `interface` for traits** |
| `enum` | All | Enumeration types |
| `constant` | All | Constants, static finals |
| `variable` | All | Variables, let bindings |
| `property` | All | Fields, attributes, members |
| `module` | Go, Java, C++, Ruby, TS | Package (Go/Java), namespace (C++), module |
| `import` | All | Import statements |
| `impl` | Rust | Impl blocks |
| `section` | Markdown | Headings |

**Text kinds:**

| Kind | Description |
|------|-------------|
| `docstring` | Documentation strings (Python, JS/TS JSDoc) |
| `comment` | Code comments |
| `string` | String literals |
| `sample` | Markdown fenced code blocks |

### Lookup tools (exact, structural)

| Tool | Input | Returns |
|---|---|---|
| `get_file_symbols` | `file` path | All symbols in that file, ordered by line |
| `get_children` | `file`, `parent` name | Direct children of a symbol |
| `get_imports` | `file` path | All imports for that file |

### Graph tools (call relationships)

| Tool | Input | Returns |
|---|---|---|
| `get_callers` | `name`, optional `kind`/`project` filters | All call sites and references to a symbol |
| `get_callees` | `caller`, optional `kind`/`project` filters | All symbols that a function calls |

### Index management

| Tool | Input | Returns |
|---|---|---|
| `flush_index` | — | Persist pending index changes to `.codeindex/` on disk |

**Design principles:**
- **Unified search** — one tool (`search`) replaces the previous 3 separate search tools (`search_symbols`, `search_files`, `search_texts`). Agents use `scope` to filter by type instead of choosing the right tool.
- Each tool maps to one query pattern on the SQLite FTS5 database
- Search is fuzzy and ranked — for discovery ("find something related to auth")
- Lookup tools are exact and structural — for navigation ("what's in this file?")
- Graph tools expose call relationships from reference indexing
- All return JSON arrays, paginated if needed
- All can scope to a specific mounted index (for monorepo/dependency queries)

**What's deliberately excluded:**
- Raw code search — the agent already has grep/rg tools
- File content reading — the agent already has file read tools
- Index management (init, refresh, configure) — the index is pre-built and auto-loaded
- Refactoring (rename, insert) — out of scope, that's an LSP concern

**Consequences:**
- Agents can query immediately — no setup calls needed
- Single unified search reduces cognitive load — agents don't need to pick between 3 search tools
- Tool descriptions are clear and non-overlapping — agents pick the right tool easily
- Unique capabilities: prose search via `scope: ["text"]`, call graph via `get_callers`/`get_callees`
- Tight surface = easier to implement, test, and document

---

## ADR-010: Host language — Rust

**Context:** The project needs a language that delivers native performance (the original motivation — Python was too slow), has first-class tree-sitter support, and produces a single distributable binary with no runtime.

**Decision:** Rust.

**Key crates:**
- `tree-sitter` — native C FFI bindings for parsing
- `rusqlite` — SQLite with FTS5, static linking
- `notify` — cross-platform file watching
- `rmcp` — MCP protocol SDK
- `serde` / `serde_json` — JSONL serialization
- `clap` — CLI argument parsing
- `grep` family (future) — embeddable ripgrep for raw code search

**Why Rust over alternatives:**

| | Rust | Go | TypeScript |
|---|---|---|---|
| Performance | Native, zero-cost | Fast, GC pauses | V8 overhead — the problem we're solving |
| tree-sitter | First-class C FFI | CGo overhead, painful for static builds | WASM or native addon |
| Single binary | Yes, no runtime | Yes, no runtime | Needs Node.js |
| ripgrep (future) | Native crates | N/A | N/A |
| SQLite + FTS5 | `rusqlite`, static | `go-sqlite3` or pure Go | `better-sqlite3`, native addon |

**Risks accepted:**
- Slower iteration speed than Go/TS — mitigated by focused scope (CLI + MCP server, not a sprawling app)
- MCP SDK (`rmcp`) is newer than the TypeScript reference impl — protocol is simple JSON-RPC over stdio, can implement transport layer ourselves if needed
- Steeper contributor learning curve — acceptable for a performance-focused tool

**Consequences:**
- Single static binary — `cargo build --release` produces one file, no runtime dependencies
- All core dependencies (tree-sitter, SQLite, file watcher) link statically
- Cross-compilation via `cross` for Linux/macOS/Windows
- Consistent with the ecosystem: ripgrep, tree-sitter, bat, fd, delta — all Rust CLI tools

**Distribution — multi-ecosystem, multi-platform:**

Release tooling: `cargo-dist` (npm, Homebrew, GitHub Releases, shell installers) + `maturin` (PyPI wheels). Same pattern as ruff, biome, esbuild.

| Channel | Install command | How |
|---|---|---|
| crates.io | `cargo install codeindex` | Native Rust |
| npm | `npx codeindex` | `@codeindex/{platform}` packages via `optionalDependencies` |
| PyPI | `pip install codeindex` | Platform-specific wheels via maturin |
| Homebrew | `brew install codeindex` | Formula generated by cargo-dist |
| GitHub Releases | Direct download | Prebuilt tarballs + checksums |
| Shell | `curl -fsSL ... \| sh` | Installer script generated by cargo-dist |

Platform targets (8 binaries):

| OS | x86_64 | aarch64 |
|---|---|---|
| Linux (glibc) | ✓ | ✓ |
| Linux (musl/static) | ✓ | ✓ |
| macOS | ✓ | ✓ (Apple Silicon) |
| Windows | ✓ | ✓ |

CI builds on GitHub Actions with native runners for each target. The `.codeindex/` format itself is plain JSONL — any language can read it directly without the binary.

---

## ADR-011: Project discovery — `.git/` boundaries

**Context:** Codeindex must handle diverse project layouts: single repos, monorepos, sibling repos cloned side by side, git submodules, and arbitrary nesting. The user launches `codeindex` once from any directory and expects everything below to be indexed.

**Decision:** One rule — **every directory containing `.git/` gets its own `.codeindex/`**.

The scanner walks the full tree downward from the launch directory. When it encounters a `.git/` (directory or file — submodules use a `.git` file), it treats that subtree as a self-contained project and places `.codeindex/` at that level.

**What the scanner does:**

| Action | Scope | Example |
|---|---|---|
| **Index** (build + watch) | Source files inside each git repo, respecting `.gitignore` | `frontend/src/**` → `frontend/.codeindex/` |
| **Mount** (read-only) | Pre-existing `.codeindex/` in dependency directories | `node_modules/foo/.codeindex/` → mounted for queries |
| **Skip** | Dependency directories themselves — not your code | `node_modules/`, `.venv/`, `target/` |

**Layouts handled uniformly:**

Single repo:
```
~/myproject/          # has .git/
  src/
  .codeindex/         ← built here
```

Sibling repos (launched from parent):
```
~/projects/           # no .git/
  frontend/           # has .git/ → frontend/.codeindex/
  backend/            # has .git/ → backend/.codeindex/
  shared-lib/         # has .git/ → shared-lib/.codeindex/
```

Git submodules:
```
~/monorepo/           # has .git/  → monorepo/.codeindex/
  vendor/libfoo/      # has .git  → vendor/libfoo/.codeindex/
  vendor/libbar/      # has .git  → vendor/libbar/.codeindex/
```

**No modes, no config, no special-casing.** The `.git/` presence is the only signal. Submodules, sibling repos, and nested repos all follow the same rule.

**Locking:** Before indexing a project, the process acquires a lockfile (`.codeindex/.lock`) using OS-level file locking (`flock`/`LockFileEx`). If the lock is already held — another process is indexing this project — skip it and mount its `.codeindex/` read-only instead. The lockfile is `.gitignore`d. This prevents conflicts when multiple `codeindex` processes overlap on the same subtree (e.g. one launched from `~/projects/` and another from `~/projects/frontend/`).

**Dependency discovery** (per ADR-003) happens within each discovered project: the server reads that project's manifests and mounts any `.codeindex/` found in its resolved dependencies.

**Consequences:**
- One launch covers everything — no need to know the layout upfront
- Each repo owns its own `.codeindex/` — committed independently
- Submodules handled without submodule-specific logic
- Sibling repos handled without workspace-specific logic
- `.gitignore` is respected per-project — each repo's ignore rules apply to its own tree

**Mount-owned walker and watcher:**

Each mount owns both its directory walker and file watcher. No global workspace-spanning walker exists.

```
Mount::walk(root)
  ├── WalkDir with follow_links(false) — no symlink loops
  ├── GitignoreBuilder — builds .gitignore rules incrementally during walk
  ├── SKIP_ENTRIES — always excludes .git, .codeindex, .vscode, .idea, etc.
  ├── notify watcher — watches directories discovered during walk
  └── emits MountEvent to handler (FileAdded, FileRemoved, DirAdded, DirRemoved, ProjectAdded)
```

**Unified event model:** Both walker (initial scan) and watcher (ongoing changes) emit the same events:

| Event | Trigger | Action |
|-------|---------|--------|
| `FileAdded` | File discovered or modified | Parse with tree-sitter, update DB |
| `FileRemoved` | File deleted | Remove from DB |
| `DirAdded` | Directory discovered or created | Add to notify watcher |
| `DirRemoved` | Directory deleted | Remove from notify watcher |
| `ProjectAdded` | `.git/` directory found | Create new Mount for subproject |

**Why mount-owned:**

1. **Correct `.gitignore` handling** — `GitignoreBuilder` accumulates rules as directories are entered. Each nested `.gitignore` extends the current ruleset.

2. **Symlink safety** — `follow_links(false)` prevents CPU spin on pnpm-style `node_modules/` with circular symlinks.

3. **Isolation** — Each mount is self-contained. Subproject discovery creates a child mount with its own walker/watcher, inheriting nothing from the parent.

4. **SKIP_ENTRIES** — Hardcoded exclusions for `.git`, `.codeindex`, `.vscode`, `.idea`, `.vs`, `.DS_Store`, etc. These are never indexed regardless of `.gitignore` content.

**Workspace root as a mount:**

The workspace root (where codeix was launched) is treated as a mount like any other. If it contains `.git/`, it gets indexed. If not, it's a container for subprojects — the mount exists but has no files to index, only subprojects to discover.

---

## Future Considerations

### Project metadata tools

Agents can search symbols but don't know *how to work with the project* — how to build, test, lint, deploy. That knowledge lives in scattered config files and docs.

**Commands extraction:**
- Extract targets/scripts from Makefile, justfile, package.json, pyproject.toml, Taskfile.yml, etc.
- Expose via `get_project_commands` MCP tool
- Open: persist in `.codeindex/meta.jsonl` or scan live at serve time? Live is simpler — few entries, cheap to read, avoids writer/reader/DB plumbing for ~20 entries.
- Open: which sources in v1? Makefile + justfile + package.json covers 90%.

**Project docs:**
- Surface README, CONTRIBUTING, CLAUDE.md paths via `get_project_docs` MCP tool
- Path-only listing — agent reads full content with its own file tools

### External API documentation — two-tier model

Projects use libraries with API docs that live outside the repo (e.g. [Nuxt UI MCP docs](https://ui.nuxt.com/docs/getting-started/ai/mcp)). An agent working on a Nuxt UI project would benefit from knowing those docs exist.

**Tier 1 — Static `.codeindex/` (default, always available):**
- Project docs extracted at index time (README, CONTRIBUTING, CLAUDE.md, etc.)
- Free, offline, no infrastructure — committed to git, works anywhere
- This is the baseline: every indexed project gets this for free

**Tier 2 — Live MCP proxy (optional, declared in manifest):**
- `index.json` gains an optional `"mcp"` field declaring upstream MCP server endpoints:
  ```json
  {
    "version": "1.0",
    "name": "my-nuxt-app",
    "mcp": {
      "nuxt-ui": { "command": "npx nuxi-mcp" },
      "supabase": { "url": "https://mcp.supabase.com" }
    }
  }
  ```
- codeix discovers dependency `.codeindex/index.json`, reads `mcp` field, optionally spawns/proxies upstream
- Hosting a live MCP server is costly — static `.codeindex/` is the better default for library authors. But for libraries that *do* offer an MCP server, codeix can aggregate them into a single endpoint

**Open questions:**
- Proxy protocol: stdio spawn (local tools) vs HTTP (remote services) — support both?
- Discovery: explicit config only, or auto-discover from package metadata?
- Security: spawning arbitrary commands from dependency manifests needs sandboxing/allowlisting
- Caching: should codeix cache upstream MCP responses to reduce latency?

### MCP resources

MCP distinguishes **tools** (parameterized queries — agent calls them with arguments) from **resources** (`resource://` URIs — agent browses and reads them). codeix currently exposes only tools (search + lookup).

**Future: expose indexed data as browsable resources:**
- `resource://codeix/files` — list all indexed files
- `resource://codeix/files/{path}` — file metadata + symbols for a specific file
- `resource://codeix/symbols/{name}` — symbol details across all files
- `resource://codeix/meta/commands` — project commands
- `resource://codeix/meta/docs` — project documentation list

**Why both tools and resources?**
- Tools are for discovery — "search for something related to auth" (fuzzy, ranked)
- Resources are for navigation — "show me this file's structure" (exact, browsable)
- Some MCP clients (IDEs, chat UIs) can render resource lists as navigable trees — tools can't provide that UX

**Open questions:**
- MCP resource templates (parameterized URIs like `files/{path}`) vs static resource list?
- Resource subscriptions for live updates in watch mode? (MCP supports `resource/subscribe`)
- Should dependency indexes be exposed as sub-resources (`resource://codeix/deps/{pkg}/...`)?

### Dependency index composition (ADR-003)

Auto-mount `.codeindex/` from resolved dependencies — declared in ADR-003 but not yet implemented.

- Read package manifests to resolve dependency locations on disk
- Look for `.codeindex/` in each resolved dependency
- Mount found indexes automatically, scope queries per-package

---

## ADR-012: Cross-platform support

**Context:** Codeix targets developers on Linux, macOS, and Windows. All core functionality must work identically across platforms.

**Decision:** Use platform-agnostic abstractions via Rust crates. No platform-specific code paths unless absolutely necessary.

**Platform-specific concerns and solutions:**

| Concern | Solution | Crate |
|---------|----------|-------|
| File paths | `std::path::Path` — handles `/` vs `\` | std |
| File watching | Cross-platform event API | `notify` |
| File locking | `flock()` on Unix, `LockFileEx()` on Windows | `fs2` |
| Gitignore | Platform-aware path matching | `ignore` |
| Symlinks | Follow by default, respect platform semantics | `ignore` |
| Line endings | Preserve as-is (no normalization) | — |
| Hidden files | `.` prefix on Unix, attribute on Windows — handled by `ignore` | `ignore` |

**Crate selection criteria:**
- Must support Linux, macOS, Windows (x86_64 and aarch64)
- Prefer crates used by ripgrep/BurntSushi ecosystem (battle-tested)
- Avoid crates with platform-specific feature flags that break builds

**Consequences:**
- Single codebase, no `#[cfg(target_os)]` sprawl
- CI tests on all three platforms (already in place)
- Same binary behavior regardless of platform

---

## Resolved Questions

- [x] **Host language**: Rust — native performance, first-class tree-sitter/SQLite/ripgrep support, single static binary
- [x] **Symbol kinds**: follow LSP `SymbolKind` taxonomy (function, method, class, interface, enum, constant, variable, property, module, etc.) + `import`. Extensible — `kind` is a string, not a closed enum.
- [x] **Imports**: included in `symbols.jsonl` as `kind: "import"`. References (usage sites) excluded — unreliable without type resolution.
- [x] **Search strategy**: all JSONL loaded into in-memory SQLite + FTS5 at serve time — one query engine for symbols, files, and text search. Raw code search left to the consumer (agent's own grep tools). Embedding `rg` as a library is a future option if needed.
- [x] **File watching**: `serve --watch` keeps index in sync on disk — commit it with your code
- [x] **MCP tools**: 8 tools — 1 discovery (explore) + 1 unified search + 3 lookup (file symbols, children, imports) + 2 graph (callers, callees) + 1 index management (flush). Zero management plumbing.
- [x] **Remote indexes**: deferred. Start local only. Future option: git-based references (`git+https://...#ref:.codeindex/`) with local caching. No dedicated registry — piggyback on git.
- [x] **File hashing**: BLAKE3 truncated to 64-bit, hex-encoded (16 chars). Change detection only — collision worst case is a missed re-index, self-heals on next edit. Birthday bound at ~4B files — safe for any project. Hex over base64 for readability/tooling (grep, jq).
