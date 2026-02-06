# Changelog

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
