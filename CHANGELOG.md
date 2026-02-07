# Changelog

## [0.2.1](https://github.com/montanetech/codeix/compare/v0.2.0...v0.2.1) (2026-02-07)


### Features

* add benchmark suite for indexing speed and search quality ([908802f](https://github.com/montanetech/codeix/commit/908802fb9ebc6de8ac616c02a4d28ab7432c2a36))
* add get_callers, get_callees, search_references MCP tools ([260d802](https://github.com/montanetech/codeix/commit/260d80287f3e6ca9c8366f27fbc3f4c9427cc488))
* add ReferenceEntry struct for tracking symbol references ([6f303bf](https://github.com/montanetech/codeix/commit/6f303bfbf12b396ed286c0b041bf300b2b7f3d9d))
* add refs table and relationship query methods to database ([b77712a](https://github.com/montanetech/codeix/commit/b77712ada22b2109bfb9a6e6de94ee84d49316ab))
* allow symbol enumeration without text query ([8380d4d](https://github.com/montanetech/codeix/commit/8380d4dfb2ecb5e8158f0df26c22446bc8b04883))
* allow symbol enumeration without text query ([#15](https://github.com/montanetech/codeix/issues/15)) ([3ef127d](https://github.com/montanetech/codeix/commit/3ef127dd5e0f9c771735ce5449f4db3cc13a300b))
* implement call and import reference extraction for Python ([6ff00a8](https://github.com/montanetech/codeix/commit/6ff00a837fa946f5f0cad59032fc704bd35236f2))
* **index:** add title and description metadata to file index ([92e2f35](https://github.com/montanetech/codeix/commit/92e2f3589a360f510115724eb3f10d9326d6f3d8))
* **index:** add title and description metadata to file index ([3ba8f39](https://github.com/montanetech/codeix/commit/3ba8f39a03b6ced54af3b0fcd26fff3a880dfdab))
* **mcp:** add relationship query tools ([92a48ed](https://github.com/montanetech/codeix/commit/92a48ed68cec66decaecdbf03f361343c38fe0f5))
* **mcp:** extract project metadata from package manifests ([c1d2b2b](https://github.com/montanetech/codeix/commit/c1d2b2b0c636021abd0a7bbc84487de33aa5966c))
* **mcp:** extract project metadata from package manifests ([#20](https://github.com/montanetech/codeix/issues/20)) ([4258f0b](https://github.com/montanetech/codeix/commit/4258f0bfa2dfc1eb64a940c36c5705d4cb9d2eb2))
* **mcp:** return code snippets in search results ([e33bde3](https://github.com/montanetech/codeix/commit/e33bde31da9879943f59f76505c358de6c1bb2e7))
* **mcp:** return code snippets in search results ([#17](https://github.com/montanetech/codeix/issues/17)) ([cea8ffb](https://github.com/montanetech/codeix/commit/cea8ffb1237bf98527a973cccf2dd646b295cf7e))
* **parser:** add tokens field for FTS search on implementation details ([72a4dac](https://github.com/montanetech/codeix/commit/72a4dac4adf6018dc8e8f57eafc98af0bf9ec587))
* **parser:** add tokens field for FTS search on implementation details ([381432e](https://github.com/montanetech/codeix/commit/381432ea490258608a1ec8e75946114cefdaaed4))
* **parser:** enable token extraction for C, C++, C#, Java, JS, Ruby, TS ([4cd30bb](https://github.com/montanetech/codeix/commit/4cd30bbc3d427a9947b34d8a34079f0a5b1eadee))
* **parser:** index markdown headings as symbols (TOC support) ([3fccc89](https://github.com/montanetech/codeix/commit/3fccc892a7732ee66dffe03e64834e4dabb9a032))
* **parser:** index markdown headings as symbols (TOC support) ([991e53f](https://github.com/montanetech/codeix/commit/991e53fca983e5cfe2f7e6debc350044a62dd348))
* update treesitter parser to return references ([e080ee9](https://github.com/montanetech/codeix/commit/e080ee918648209c814f203835bd0620282dc1a8))


### Bug Fixes

* **db:** make FTS5 conditional to reduce memory in build mode ([4d7d0b8](https://github.com/montanetech/codeix/commit/4d7d0b816bbcc34926c126ef937f542872a09a8e))
* **parser:** add depth limiting to prevent stack overflow ([e7eb425](https://github.com/montanetech/codeix/commit/e7eb425a47ecca35c4f587ed25afb473560fcfe5))
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
