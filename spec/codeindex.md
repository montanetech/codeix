# `.codeindex` Format Specification

**Version:** 1.0 (draft)

## Overview

`.codeindex/` is an open, portable format for describing the structure of a source code project. It captures files, symbols (definitions + imports), and text content (comments, docstrings, string literals) in a set of plain-text files designed for git storage and tool interoperability.

The format is language-agnostic, tool-agnostic, and requires no runtime to read. Any tool that can parse JSON can consume a `.codeindex/`.

## Directory layout

A `.codeindex/` directory lives at the root of a project (typically alongside `.git/`):

```
.codeindex/
  index.json        # manifest (required)
  files.jsonl       # file registry (required)
  symbols.jsonl     # symbol index (required, may be empty)
  texts.jsonl       # text content index (required, may be empty)
```

All four files are required. `symbols.jsonl` and `texts.jsonl` may be empty files (zero lines) if the project has no parseable source files.

## File format conventions

- **JSONL** (JSON Lines): one JSON object per line, no trailing commas, no multi-line objects
- **Encoding:** UTF-8, no BOM
- **Line endings:** LF (`\n`)
- **Sorting:** each JSONL file is sorted by a stable key (documented per file below)
- **Determinism:** identical source must produce identical output — no timestamps, no random IDs, no non-deterministic iteration

## `index.json` — manifest

A single JSON object describing the index.

```json
{
  "version": "1.0",
  "name": "my-project",
  "root": ".",
  "languages": ["python", "typescript"]
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `version` | string | yes | Format version. Currently `"1.0"`. |
| `name` | string | yes | Project name (typically from package manifest or directory name). |
| `root` | string | yes | Relative path from `.codeindex/` to the project root. Always `"."` when `.codeindex/` is at the project root. |
| `languages` | string[] | yes | List of languages found in the project. Lowercase, e.g. `"python"`, `"rust"`, `"typescript"`. Empty array if no recognized languages. |

Schema: [`index.schema.json`](index.schema.json)

## `files.jsonl` — file registry

One line per source file tracked by the index. Includes all files in the project that are not ignored (respecting `.gitignore`), not just files with symbols.

**Sorted by:** `path` (lexicographic)

```jsonl
{"path":"src/main.py","lang":"python","hash":"a1b2c3d4e5f6a7b8","lines":142}
{"path":"src/utils/helpers.py","lang":"python","hash":"b2c3d4e5f6a7b8c9","lines":87}
{"path":"pytest.ini","lang":null,"hash":"c3d4e5f6a7b8c9d0","lines":12}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Relative path from project root. Forward slashes (`/`) on all platforms. |
| `lang` | string \| null | yes | Detected language, lowercase. `null` for unrecognized file types. |
| `hash` | string | yes | BLAKE3 content hash, truncated to 64 bits, hex-encoded (16 characters). Used for change detection only. |
| `lines` | integer | yes | Total line count of the file. |

Schema: [`files.schema.json`](files.schema.json)

### Language identifiers

Language values are lowercase strings. Languages with tree-sitter symbol extraction: `"python"`, `"rust"`, `"javascript"`, `"typescript"`, `"tsx"`, `"go"`, `"java"`, `"c"`, `"cpp"`, `"ruby"`, `"csharp"`.

Single File Component formats (`"vue"`, `"svelte"`, `"astro"`) are preprocessed to extract embedded script blocks, which are then parsed as JavaScript or TypeScript.

The set is open — any string is valid. Consumers should handle unknown languages gracefully.

## `symbols.jsonl` — symbol index

One line per symbol definition or import. Captures the structural skeleton of the codebase: what's defined where, with optional signatures and parent relationships.

**Sorted by:** `file` (lexicographic), then `line[0]` (ascending)

```jsonl
{"file":"src/main.py","name":"os","kind":"import","line":[1,1]}
{"file":"src/main.py","name":"utils.parse","kind":"import","line":[2,2],"alias":"parse"}
{"file":"src/main.py","name":"Config","kind":"class","line":[22,45]}
{"file":"src/main.py","name":"Config.__init__","kind":"method","line":[23,30],"parent":"Config","sig":"def __init__(self, path: str, debug: bool = False)"}
{"file":"src/main.py","name":"main","kind":"function","line":[48,60],"sig":"def main(args: list[str]) -> int"}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `file` | string | yes | Relative path to the source file (matches a `path` in `files.jsonl`). |
| `name` | string | yes | Fully qualified symbol name within the file. For nested symbols, dot-separated: `"Config.__init__"`. For imports, the imported module/name: `"os"`, `"utils.parse"`. |
| `kind` | string | yes | Symbol kind. See [Symbol kinds](#symbol-kinds) below. |
| `line` | [integer, integer] | yes | Start and end line numbers (1-based, inclusive). |
| `parent` | string | no | Name of the parent symbol, if nested (e.g. `"Config"` for a method inside a class). Omitted for top-level symbols. |
| `sig` | string | no | Raw declaration text extracted from the AST — parameters, return type, as it appears in source. No normalization across languages. Omitted when not applicable (e.g. imports, variables). |
| `alias` | string | no | Local alias for imports (e.g. `import utils.parse as parse` → `alias: "parse"`). Omitted when import has no alias. |
| `visibility` | string | no | Symbol visibility. See [Visibility](#visibility) below. Omitted when not determinable. |

Schema: [`symbols.schema.json`](symbols.schema.json)

### Symbol kinds

Symbol kinds follow the [LSP `SymbolKind`](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#symbolKind) taxonomy where applicable, plus `"import"` for import statements.

Common values:

| Kind | Description |
|---|---|
| `"function"` | Function definition |
| `"method"` | Method definition (inside a class/struct/impl) |
| `"class"` | Class definition |
| `"interface"` | Interface / trait / protocol definition |
| `"enum"` | Enum definition |
| `"struct"` | Struct definition |
| `"constant"` | Constant / static value |
| `"variable"` | Variable / let binding at module scope |
| `"property"` | Property / field / attribute |
| `"module"` | Module / namespace definition |
| `"type_alias"` | Type alias / typedef |
| `"import"` | Import statement |

The `kind` field is an open string — not a closed enum. Producers may emit additional kinds for language-specific constructs (e.g. `"decorator"`, `"macro"`, `"trait_impl"`). Consumers should handle unknown kinds gracefully.

### Nesting

Symbols use a flat structure with a `parent` field for nesting, rather than nested objects. This keeps the JSONL format simple and each line independently parseable.

A class with methods:
```jsonl
{"file":"src/app.py","name":"App","kind":"class","line":[10,50]}
{"file":"src/app.py","name":"App.__init__","kind":"method","line":[11,15],"parent":"App","sig":"def __init__(self)"}
{"file":"src/app.py","name":"App.run","kind":"method","line":[17,50],"parent":"App","sig":"def run(self) -> None"}
```

To reconstruct a tree: group by `file`, then match `parent` fields to `name` fields.

### Visibility

The optional `visibility` field tags symbols with their access level. This enables consumers to filter by API surface without losing internal details — the full index is preserved, and the consumer decides what to show.

| Value | Meaning | Examples |
|---|---|---|
| `"public"` | Part of the public API surface | Rust `pub`, Python no underscore, TS `export`, Java `public` |
| `"internal"` | Visible within the package/crate/module, not exported | Rust `pub(crate)`, Python `_prefix`, TS unexported, Java package-private |
| `"private"` | Visible only within the containing scope | Rust no modifier, Python `__prefix`, Java `private` |

Omitted when visibility cannot be determined — e.g. languages without explicit visibility modifiers, or when the parser cannot infer it. Consumers should treat missing `visibility` as unknown, not as any specific access level.

```jsonl
{"file":"src/lib.rs","name":"Client","kind":"struct","line":[10,50],"visibility":"public"}
{"file":"src/lib.rs","name":"Client.connect","kind":"method","line":[12,25],"parent":"Client","visibility":"public","sig":"pub fn connect(&self, url: &str) -> Result<()>"}
{"file":"src/lib.rs","name":"Client.retry_internal","kind":"method","line":[27,40],"parent":"Client","visibility":"private","sig":"fn retry_internal(&self, attempts: u32)"}
```

This keeps published indexes complete rather than pre-filtered. A library ships its full `.codeindex/` — consumers browse the public API by default, but can dig into internals when debugging or exploring.

## `texts.jsonl` — text content index

One line per comment, docstring, or string literal extracted from source files. Enables full-text search over human-written prose — TODOs, documentation, error messages — without noise from code.

**Sorted by:** `file` (lexicographic), then `line[0]` (ascending)

```jsonl
{"file":"src/main.py","kind":"docstring","line":[15,18],"text":"Validates user credentials against the database.","parent":"authenticate"}
{"file":"src/main.py","kind":"comment","line":[45,45],"text":"TODO: add rate limiting"}
{"file":"src/main.py","kind":"string","line":[22,22],"text":"Invalid credentials for user: %s"}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `file` | string | yes | Relative path to the source file. |
| `kind` | string | yes | One of: `"comment"`, `"docstring"`, `"string"`. |
| `line` | [integer, integer] | yes | Start and end line numbers (1-based, inclusive). |
| `text` | string | yes | The extracted text content. Leading/trailing whitespace trimmed. Multi-line text joined with `\n`. |
| `parent` | string | no | Name of the enclosing symbol, if any (e.g. a docstring's parent is the function it documents). Omitted for file-level comments or orphaned text. |

Schema: [`texts.schema.json`](texts.schema.json)

### Filtering

Producers should exclude trivial text entries:
- Empty strings (`""`)
- Single-character strings
- Whitespace-only strings
- Auto-generated boilerplate (e.g. `"use strict"`, shebang lines)

The exact filtering rules are implementation-defined. The goal is to keep the index useful for search — not to capture every string literal.

## Hashing

File hashes in `files.jsonl` use **BLAKE3**, truncated to 64 bits, hex-encoded (16 lowercase characters).

The hash covers the file's byte content (not its path or metadata). It is used for change detection only — determining whether a file needs re-indexing. It is not used for content addressing.

Collision consequences are benign: a collision means a changed file is not re-indexed on one build cycle. It self-heals on the next edit.

## Sorting and determinism

All JSONL files are sorted by their primary key:

| File | Sort key |
|---|---|
| `files.jsonl` | `path` (lexicographic) |
| `symbols.jsonl` | `file` (lexicographic), then `line[0]` (ascending) |
| `texts.jsonl` | `file` (lexicographic), then `line[0]` (ascending) |

Within a sort key tie (same file, same start line), the order is stable but implementation-defined.

Identical source files must produce identical index output. No timestamps, random IDs, or iteration-order-dependent values.

## Line numbers

All line numbers are **1-based** and **inclusive**. A symbol spanning lines 22 through 45 is represented as `"line": [22, 45]`. A single-line symbol on line 10 is `"line": [10, 10]`.

## Path conventions

All paths are **relative to the project root** (the directory containing `.codeindex/`). Forward slashes (`/`) are used on all platforms. No leading `./`.

Examples:
- `"src/main.py"` (correct)
- `"./src/main.py"` (incorrect)
- `"src\\main.py"` (incorrect)

## Versioning

The format version in `index.json` follows a simple `MAJOR.MINOR` scheme:

- **Major bump:** breaking changes — consumers of the old version cannot read the new format
- **Minor bump:** additive changes — new optional fields, new symbol kinds. Old consumers can still read the format (they ignore unknown fields).

Consumers should check `version` and reject indexes with an unsupported major version. Unknown fields in JSONL entries should be ignored, not rejected.

## Distribution

`.codeindex/` is a portable artifact. It can be distributed through multiple channels:

### Git repository (primary)

Committed at the project root, alongside `.git/`. This is the default and recommended distribution method.

```
my-project/
  .git/
  .codeindex/       # committed with the code
  src/
  package.json
```

Anyone who clones the repo gets the index. No build step, no re-indexing.

### Published packages

Library authors can include `.codeindex/` in their published packages:

| Ecosystem | Where | How |
|---|---|---|
| npm | `node_modules/foo/.codeindex/` | Include in `files` field of `package.json` |
| PyPI | `.venv/lib/.../foo/.codeindex/` | Include in wheel via `MANIFEST.in` or `pyproject.toml` |
| crates.io | Cargo downloads | Include via `include` in `Cargo.toml` |

Consumers of the library get instant symbol navigation of their dependencies — no indexing required.

### HTTP discovery (future)

For web-accessible projects, a `.well-known/codeindex/` convention enables tool discovery over HTTP:

```
https://example.com/.well-known/codeindex/index.json
https://example.com/.well-known/codeindex/symbols.jsonl
```

This allows AI agents and tools to fetch a project's index without cloning the repository. The directory structure and file formats are identical to the git-committed version.

Raw git hosting already provides this implicitly:
```
https://raw.githubusercontent.com/org/repo/main/.codeindex/symbols.jsonl
```

A formal `.well-known/` registration is deferred until the format sees adoption.

## Scope and visibility

Indexes should include **all** symbols, tagged with a [`visibility`](#visibility) field. This avoids lossy pre-filtering — consumers decide what to show at query time.

| Use case | How |
|---|---|
| Library consumer browsing the API | Filter to `visibility: "public"` |
| Debugging a dependency | Show all symbols, including `"private"` and `"internal"` |
| Own project development | No filter — everything is relevant |

Published packages ship the full index. The `visibility` field lets consumers focus on the public surface without losing the ability to explore internals when needed.

### Remote APIs (future)

The open `kind` field naturally extends to non-source-code constructs. A REST, GraphQL, or gRPC service could publish a `.codeindex/` describing its API surface:

```jsonl
{"file":"api/users.py","name":"GET /users","kind":"endpoint","line":[15,30],"sig":"GET /users?page=int&limit=int -> UserList"}
{"file":"schema.graphql","name":"Query.user","kind":"query","line":[5,8],"sig":"user(id: ID!): User"}
{"file":"proto/service.proto","name":"UserService.GetUser","kind":"rpc","line":[12,14],"sig":"GetUser(GetUserRequest) returns (User)"}
```

This is not part of the 1.0 spec — existing schema formats (OpenAPI, GraphQL SDL, Protobuf) already serve this role. But the format accommodates it without changes: `kind` is an open string, and `sig` captures the declaration naturally.
