# Changelog

## [0.4.1](https://github.com/montanetech/codeix/compare/v0.4.0...v0.4.1) (2026-02-11)


### Features

* add MCP registry manifest (server.json) ([bde48d2](https://github.com/montanetech/codeix/commit/bde48d2de60f06f229e9c48c6bb806d6f85ed3e2))

## [0.4.0](https://github.com/montanetech/codeix/compare/v0.3.0...v0.4.0) (2026-02-11)


### ⚠ BREAKING CHANGES

* **mcp:** remove redundant get_imports tool

### Features

* add MCP Registry publishing to release workflow ([#69](https://github.com/montanetech/codeix/issues/69)) ([e82f4de](https://github.com/montanetech/codeix/commit/e82f4de8d17bd0135cf9d10a403ab96cf6c8eb54))
* **mcp:** add visibility filter to MCP tools ([#71](https://github.com/montanetech/codeix/issues/71)) ([bb3f9cb](https://github.com/montanetech/codeix/commit/bb3f9cbd2e07476df1a3df95d0a73912bfdeb46f))
* **mcp:** add visibility filter to MCP tools ([#71](https://github.com/montanetech/codeix/issues/71)) ([f7f840b](https://github.com/montanetech/codeix/commit/f7f840b42d4521691897a8cdb995815900eecad5))
* **mcp:** remove redundant get_imports tool ([c6ec622](https://github.com/montanetech/codeix/commit/c6ec62265dd61075df02ad28db6c05bf1f0d69fc))
* **parser:** harden parsers to handle errors gracefully ([87666fb](https://github.com/montanetech/codeix/commit/87666fbca0db8f86468138d245252903faa24e55))
* **parser:** use __all__ for Python visibility detection ([778b0fc](https://github.com/montanetech/codeix/commit/778b0fcf65035dc0eb9557e83bcbf4e766af9a9b))
* **parser:** use __all__ for Python visibility detection ([#73](https://github.com/montanetech/codeix/issues/73)) ([f5a3a70](https://github.com/montanetech/codeix/commit/f5a3a708807cd244481fe96a293615f464b4426f))
* **rust:** parse symbols inside macro invocations ([1ed4530](https://github.com/montanetech/codeix/commit/1ed45306f0b4bbc49154affbb39793052795cd09))
* **search:** add BM25 weighted columns for better relevance ([756d207](https://github.com/montanetech/codeix/commit/756d207278ebb46d3aead96b758b8d0b9dc1d9ab))
* **search:** add kind to FTS content for natural queries ([b0c06c5](https://github.com/montanetech/codeix/commit/b0c06c570d216c252be60327845d51b0965f583c))
* **search:** add pipe syntax for OR queries ([82bfe2d](https://github.com/montanetech/codeix/commit/82bfe2dfdb2931a484ef7611d15ac072d05edeac))
* **search:** allow multiple kinds + improve docs ([5c4ffdb](https://github.com/montanetech/codeix/commit/5c4ffdb164d9c89a22893e4cbf5626f1aca93912))
* **search:** glob patterns for get_file_symbols ([e16fdd4](https://github.com/montanetech/codeix/commit/e16fdd45d9564018d3de17cc73890f9fadd42b27))


### Bug Fixes

* **c:** classify functions returning pointers correctly ([084840e](https://github.com/montanetech/codeix/commit/084840e01c4683dafe0685b36ccfca1a507aa1ee))
* **db:** support base name matching in get_callers and get_callees ([f0974ec](https://github.com/montanetech/codeix/commit/f0974ecb8df6dd6d814ca49cf8c5affac3df49df))
* **db:** support base name matching in get_callers and get_callees ([5f982a7](https://github.com/montanetech/codeix/commit/5f982a77174cdc2a9b86a762ad1a44f2ec49aa79))
* enforce minimum context + hide redundant fields ([6fdac27](https://github.com/montanetech/codeix/commit/6fdac2755d9f151d62c042c5396294858f480793))
* hide tokens field from serialized output ([aea11d3](https://github.com/montanetech/codeix/commit/aea11d3e72cb97342daa293dbe0a19feb1800fb2))
* **test:** canonicalize paths in macOS CI tests ([2fb1ede](https://github.com/montanetech/codeix/commit/2fb1edeb21712cf87490978541e5a08449856f55))
* **watcher:** emit ProjectRemoved event when subproject deleted ([657ced3](https://github.com/montanetech/codeix/commit/657ced380b9c36b72faf1ac753abaf41e43b6c46))
* **watcher:** emit ProjectRemoved event when subproject deleted ([#61](https://github.com/montanetech/codeix/issues/61)) ([3f78fad](https://github.com/montanetech/codeix/commit/3f78fad459bb91ad013c69043c3ec395df12d4ef))

## [0.3.0](https://github.com/montanetech/codeix/compare/v0.2.0...v0.3.0) (2026-02-10)


### ⚠ BREAKING CHANGES

* MCP tool API has changed significantly:
    - `search_symbols`, `search_files`, `search_texts` replaced by unified `search` tool
    - `list_projects` replaced by `explore` tool
    - `get_symbol_children` renamed to `get_children`
    - CLI now uses `-r/--root` option instead of positional path argument
    - Parameter names unified across all tools

### Features

* **#10:** explicit flush with trigger file mechanism ([adcf769](https://github.com/montanetech/codeix/commit/adcf7692ba1632533b1f1364f715c87efee19b08))
* **#10:** explicit flush with trigger file mechanism ([694466c](https://github.com/montanetech/codeix/commit/694466c9d5da6a69309c0a5162c1819909da7eaa))
* **#36:** mount owns walker and watcher, single-walk strategy ([3bbadd5](https://github.com/montanetech/codeix/commit/3bbadd5ff874af621701f2c090077a07dc72e919))
* add benchmark suite for indexing speed and search quality ([908802f](https://github.com/montanetech/codeix/commit/908802fb9ebc6de8ac616c02a4d28ab7432c2a36))
* add format parameter for human-readable vs JSON output ([3ffac86](https://github.com/montanetech/codeix/commit/3ffac86e60b6167f23aa5be76db3558bba099f46))
* add format parameter for human-readable vs JSON output ([#51](https://github.com/montanetech/codeix/issues/51)) ([501db9b](https://github.com/montanetech/codeix/commit/501db9b69028eb52f15e3aa65187ac02c7be9b3b))
* add get_callers, get_callees, search_references MCP tools ([260d802](https://github.com/montanetech/codeix/commit/260d80287f3e6ca9c8366f27fbc3f4c9427cc488))
* add interactive query REPL ([a83284c](https://github.com/montanetech/codeix/commit/a83284caaa471216c6057e5eb47dc2c2e26e2624))
* add interactive query REPL (closes [#39](https://github.com/montanetech/codeix/issues/39)) ([17da5a8](https://github.com/montanetech/codeix/commit/17da5a8fc96882c79c926ca3d0d28d0640e1c07f))
* add ReferenceEntry struct for tracking symbol references ([6f303bf](https://github.com/montanetech/codeix/commit/6f303bfbf12b396ed286c0b041bf300b2b7f3d9d))
* add refs table and relationship query methods to database ([b77712a](https://github.com/montanetech/codeix/commit/b77712ada22b2109bfb9a6e6de94ee84d49316ab))
* allow symbol enumeration without text query ([8380d4d](https://github.com/montanetech/codeix/commit/8380d4dfb2ecb5e8158f0df26c22446bc8b04883))
* allow symbol enumeration without text query ([#15](https://github.com/montanetech/codeix/issues/15)) ([3ef127d](https://github.com/montanetech/codeix/commit/3ef127dd5e0f9c771735ce5449f4db3cc13a300b))
* **explore:** add explore tool with budget-based file capping ([0eb28a3](https://github.com/montanetech/codeix/commit/0eb28a3914f6764615df039251b0c82b2d1c016e))
* implement call and import reference extraction for Python ([6ff00a8](https://github.com/montanetech/codeix/commit/6ff00a837fa946f5f0cad59032fc704bd35236f2))
* **index:** add title and description metadata to file index ([92e2f35](https://github.com/montanetech/codeix/commit/92e2f3589a360f510115724eb3f10d9326d6f3d8))
* **index:** add title and description metadata to file index ([3ba8f39](https://github.com/montanetech/codeix/commit/3ba8f39a03b6ced54af3b0fcd26fff3a880dfdab))
* **makefile:** add standard targets ([f9b63c1](https://github.com/montanetech/codeix/commit/f9b63c182cbfe0b7c36b1b319abea6281fb8172e))
* **mcp:** add explore tool for project structure discovery ([#47](https://github.com/montanetech/codeix/issues/47)) ([8b7ae55](https://github.com/montanetech/codeix/commit/8b7ae55544b52a1973d6594a67066e8f8347e446))
* **mcp:** add relationship query tools ([92a48ed](https://github.com/montanetech/codeix/commit/92a48ed68cec66decaecdbf03f361343c38fe0f5))
* **mcp:** extract project metadata from package manifests ([c1d2b2b](https://github.com/montanetech/codeix/commit/c1d2b2b0c636021abd0a7bbc84487de33aa5966c))
* **mcp:** extract project metadata from package manifests ([#20](https://github.com/montanetech/codeix/issues/20)) ([4258f0b](https://github.com/montanetech/codeix/commit/4258f0bfa2dfc1eb64a940c36c5705d4cb9d2eb2))
* **mcp:** return code snippets in search results ([e33bde3](https://github.com/montanetech/codeix/commit/e33bde31da9879943f59f76505c358de6c1bb2e7))
* **mcp:** return code snippets in search results ([#17](https://github.com/montanetech/codeix/issues/17)) ([cea8ffb](https://github.com/montanetech/codeix/commit/cea8ffb1237bf98527a973cccf2dd646b295cf7e))
* **parser/c:** add reference extraction for calls, includes, type annotations ([#41](https://github.com/montanetech/codeix/issues/41)) ([aea1f02](https://github.com/montanetech/codeix/commit/aea1f021685fc7cf28b1e1519b55cc0070faf3bf))
* **parser/cpp:** add reference extraction for calls, includes, type annotations ([#41](https://github.com/montanetech/codeix/issues/41)) ([15cb6a9](https://github.com/montanetech/codeix/commit/15cb6a9e9349e205c2a0d88479341c8fb31159f3))
* **parser/csharp:** add reference extraction for calls, usings, type annotations ([#41](https://github.com/montanetech/codeix/issues/41)) ([8ea9392](https://github.com/montanetech/codeix/commit/8ea9392aff7fc8e2ee8c0fe66b3d834df99f9f85))
* **parser/go:** add reference extraction for calls, imports, type annotations ([#41](https://github.com/montanetech/codeix/issues/41)) ([d3b3349](https://github.com/montanetech/codeix/commit/d3b3349fa1cdf476382b2c2b726cc4ad38f4af14))
* **parser/java:** add reference extraction for calls, imports, type annotations ([#41](https://github.com/montanetech/codeix/issues/41)) ([a64418f](https://github.com/montanetech/codeix/commit/a64418fd23f20a3a69f809babd7c2a9f4a2b7740))
* **parser/javascript:** add reference extraction for calls and imports ([#41](https://github.com/montanetech/codeix/issues/41)) ([ef7d533](https://github.com/montanetech/codeix/commit/ef7d5330deebce5a274a1a1a714461e1609379ff))
* **parser/ruby:** add reference extraction for calls and requires ([#41](https://github.com/montanetech/codeix/issues/41)) ([b3e3659](https://github.com/montanetech/codeix/commit/b3e36591df312e2008337e9caa8e1c68adf84040))
* **parser/rust:** add reference extraction for calls, imports, type annotations ([#41](https://github.com/montanetech/codeix/issues/41)) ([1cfb39f](https://github.com/montanetech/codeix/commit/1cfb39f4e5acd290de52786322023d664946b07e))
* **parser/typescript:** add reference extraction for calls, imports, type annotations ([#41](https://github.com/montanetech/codeix/issues/41)) ([c8cca10](https://github.com/montanetech/codeix/commit/c8cca10888a83a4cea43d1ba985a4b139b2a38c3))
* **parser:** add reference extraction to all language parsers ([#41](https://github.com/montanetech/codeix/issues/41)) ([90749d0](https://github.com/montanetech/codeix/commit/90749d0b33992844f8490265a462a48095254a62))
* **parser:** add tokens field for FTS search on implementation details ([72a4dac](https://github.com/montanetech/codeix/commit/72a4dac4adf6018dc8e8f57eafc98af0bf9ec587))
* **parser:** add tokens field for FTS search on implementation details ([381432e](https://github.com/montanetech/codeix/commit/381432ea490258608a1ec8e75946114cefdaaed4))
* **parser:** enable token extraction for C, C++, C#, Java, JS, Ruby, TS ([4cd30bb](https://github.com/montanetech/codeix/commit/4cd30bbc3d427a9947b34d8a34079f0a5b1eadee))
* **parser:** index markdown headings as symbols (TOC support) ([3fccc89](https://github.com/montanetech/codeix/commit/3fccc892a7732ee66dffe03e64834e4dabb9a032))
* **parser:** index markdown headings as symbols (TOC support) ([991e53f](https://github.com/montanetech/codeix/commit/991e53fca983e5cfe2f7e6debc350044a62dd348))
* unified search API and reference extraction ([2d43fad](https://github.com/montanetech/codeix/commit/2d43fad5e7e92d3907340a9b52f2eeb23db4a936))
* unified search tool with hybrid FTS architecture (closes [#48](https://github.com/montanetech/codeix/issues/48)) ([4f47c82](https://github.com/montanetech/codeix/commit/4f47c82b7dd212c9705d3db42efcbcd9a7c07bce))
* unified search tool with hybrid FTS architecture (closes [#48](https://github.com/montanetech/codeix/issues/48)) ([1923fc6](https://github.com/montanetech/codeix/commit/1923fc6727e614471e0f4b32c796ff9c58adc820))
* update treesitter parser to return references ([e080ee9](https://github.com/montanetech/codeix/commit/e080ee918648209c814f203835bd0620282dc1a8))


### Bug Fixes

* **#36:** mount owns walker and watcher to fix CPU spin ([3a70c21](https://github.com/montanetech/codeix/commit/3a70c21773ea91d331d4588f5d11bf047b80b083))
* **bench:** handle Claude CLI streaming buffer errors with fallback ([7654963](https://github.com/montanetech/codeix/commit/7654963b3acbb3cb4ac337261f23406e1a3da8fc))
* **bench:** replace unsupported repos with real projects ([fecaed1](https://github.com/montanetech/codeix/commit/fecaed11d9acf6487a847cd920d8f33a88e42afb))
* **bench:** update build command to use -r flag ([1d62c52](https://github.com/montanetech/codeix/commit/1d62c5234084e35fe9892c4e0378b6724cf494a1))
* **bench:** update search questions for supported repos ([539a40a](https://github.com/montanetech/codeix/commit/539a40ad8e3758bb8cfe7d420ab5c178e7084605))
* **db:** make FTS5 conditional to reduce memory in build mode ([4d7d0b8](https://github.com/montanetech/codeix/commit/4d7d0b816bbcc34926c126ef937f542872a09a8e))
* **parser:** add depth limiting to prevent stack overflow ([e7eb425](https://github.com/montanetech/codeix/commit/e7eb425a47ecca35c4f587ed25afb473560fcfe5))
* **parser:** handle multi-byte UTF-8 chars in markdown headings ([c95d6dc](https://github.com/montanetech/codeix/commit/c95d6dc45292041efeb76d67835bff4785596af8))
* **parser:** handle multi-byte UTF-8 chars in markdown headings ([#52](https://github.com/montanetech/codeix/issues/52)) ([d2e7a82](https://github.com/montanetech/codeix/commit/d2e7a822bc4860b7ea4b7716d9320a47b06f9597))
* prevent crash on large repositories (issue [#24](https://github.com/montanetech/codeix/issues/24)) ([a665c19](https://github.com/montanetech/codeix/commit/a665c19cadd7a0e55bc7a5915f2f41673f9bb667))


### Performance Improvements

* **writer:** remove redundant memory copies during export ([eedc265](https://github.com/montanetech/codeix/commit/eedc2651293c796797a47e1175e17b1c3ed7fff0))

## [0.2.0](https://github.com/montanetech/codeix/compare/v0.1.8...v0.2.0) (2026-02-06)


### ⚠ BREAKING CHANGES

* Database schema changed with new project column. Existing indexes need to be rebuilt.

### Features

* **db:** add project column to schema ([de8a54f](https://github.com/montanetech/codeix/commit/de8a54f3842387d7470242290dd63031d81e0233))
* **handler:** add subproject discovery with MountTable integration ([6b1b610](https://github.com/montanetech/codeix/commit/6b1b610834feb9791ba5e44f29e12e405258ba0f))
* **mcp:** add list_projects tool and project filter to search tools ([df19cb4](https://github.com/montanetech/codeix/commit/df19cb4fd61b088164549bc7d6e9cf7cdb9dd1f0))
* multi-repo mount table with subproject discovery ([5daebed](https://github.com/montanetech/codeix/commit/5daebedc9cb2186376e9fc881a4c684abd4c00d4))
* multi-repo support with MountTable ([1ab01f6](https://github.com/montanetech/codeix/commit/1ab01f68159ed29e77e0f61f06d4a6f186ea19c2))
* **scanner:** add MountTable with flock locking ([a70f833](https://github.com/montanetech/codeix/commit/a70f833963689a3c66a68d7601b001fb36f120af))


### Performance Improvements

* reduce readlink syscalls in hot paths ([27582e6](https://github.com/montanetech/codeix/commit/27582e6e18322899be23face76dece937d7134a3))

## [0.1.8](https://github.com/montanetech/codeix/compare/v0.1.7...v0.1.8) (2026-02-06)


### Features

* add codeix.dev website ([0967e32](https://github.com/montanetech/codeix/commit/0967e3246c2cc5142d7b67167a0aba2d7a37f732))


### Bug Fixes

* upgrade notify to 9.0.0-rc.1 with EventKindMask::CORE ([4924dac](https://github.com/montanetech/codeix/commit/4924dace1d48897f73cb3ba7e72cba9aca65ca0f))
* use matched_path_or_any_parents for gitignore checks ([6a2eaf3](https://github.com/montanetech/codeix/commit/6a2eaf34ca1ed3edd7bcca4c4cac13425126cd19))

## [0.1.7](https://github.com/montanetech/codeix/compare/v0.1.6...v0.1.7) (2026-02-05)


### Bug Fixes

* npm OIDC trusted publishing and crates.io auth token ([75db693](https://github.com/montanetech/codeix/commit/75db69337dcb0341b41e528e05de982f784a0afd))

## [0.1.6](https://github.com/montanetech/codeix/compare/v0.1.5...v0.1.6) (2026-02-05)


### Bug Fixes

* include README.md in npm and PyPI packages ([ca32fce](https://github.com/montanetech/codeix/commit/ca32fce723702203899ada2457f5275c4c379780))

## [0.1.5](https://github.com/montanetech/codeix/compare/v0.1.4...v0.1.5) (2026-02-05)


### Bug Fixes

* use trusted publishers (OIDC) for npm, PyPI, and crates.io ([ee579bc](https://github.com/montanetech/codeix/commit/ee579bc74c4e142c2ba07988aec9ed215fc9cc42))
* use valid PyPI classifier ([341db80](https://github.com/montanetech/codeix/commit/341db8072ea5dfd6025af71c7074a0016e724a36))

## [0.1.4](https://github.com/montanetech/codeix/compare/v0.1.3...v0.1.4) (2026-02-05)


### Bug Fixes

* add actions:write permission to release workflow for workflow_dispatch ([53c5f60](https://github.com/montanetech/codeix/commit/53c5f601b2c5c9c43c152fb8a57dd05db0b9d4e9))

## [0.1.3](https://github.com/montanetech/codeix/compare/v0.1.2...v0.1.3) (2026-02-05)


### Bug Fixes

* chain release workflow from release-please via workflow_call ([1476c62](https://github.com/montanetech/codeix/commit/1476c62b36c02e912b19bfb80a0b28123f2653ef))

## [0.1.2](https://github.com/montanetech/codeix/compare/v0.1.1...v0.1.2) (2026-02-05)


### Bug Fixes

* drop x86_64-apple-darwin target (macos-13 runner retired) ([60f383f](https://github.com/montanetech/codeix/commit/60f383fcd59928ad3bb90f4db2cd59d7de51ff97))
* remove component prefix from release-please tags ([0ee659e](https://github.com/montanetech/codeix/commit/0ee659e723f7f7fbcd8ee09bf94e57b4df8f7f2c))

## [0.1.1](https://github.com/montanetech/codeix/compare/codeix-v0.1.0...codeix-v0.1.1) (2026-02-05)


### Features

* add full language support for 10 languages ([3bdd4eb](https://github.com/montanetech/codeix/commit/3bdd4ebe76159ccd8cd1b01410babb5857bd394b))
* build command with progress output ([6dfe59c](https://github.com/montanetech/codeix/commit/6dfe59ce246d4222eb4313e211510ee7f9a7ee2e))
* file scanner and hasher ([210f5cb](https://github.com/montanetech/codeix/commit/210f5cb31fc21a40d917ff29bee87aa525332636))
* file watcher with incremental indexing ([5e0f7f7](https://github.com/montanetech/codeix/commit/5e0f7f782cb97a501fae90f6a0665616b58f79c3))
* index format and I/O ([7b30e5c](https://github.com/montanetech/codeix/commit/7b30e5c3e49443b04073079e6ff0b5e8967625c9))
* MCP server with 6 tools ([a53c921](https://github.com/montanetech/codeix/commit/a53c921f3627098c95ce8ff533d06ec8a2047144))
* project scaffold and dependencies ([5d7f5af](https://github.com/montanetech/codeix/commit/5d7f5afc4633de003e8bb6c9cd2e34eb55d0b1ac))
* serve command and CLI entrypoint ([87d0194](https://github.com/montanetech/codeix/commit/87d01943ca0336066a15dcdb9b6456a26b7984cc))
* SQLite FTS5 search database ([a0e7357](https://github.com/montanetech/codeix/commit/a0e73572350452e15b62b6ce75368f47c5f926b5))
* tree-sitter parser with Rust extraction ([f705a31](https://github.com/montanetech/codeix/commit/f705a310e1fd48ff69f0040e44984f179c0bb2c2))
