# Changelog

## [0.3.0](https://github.com/asvarnon/context-forge/compare/v0.2.0...v0.3.0) (2026-04-01)


### Features

* add install scripts for all 3 platforms ([1a59d90](https://github.com/asvarnon/context-forge/commit/1a59d909cacd7dc4eb80d3ab002118384fe7bd80))
* **ci:** add napi build verification and release workflow ([0a5332a](https://github.com/asvarnon/context-forge/commit/0a5332a87bc0f5899be36397c57305b5059e10dc)), closes [#8](https://github.com/asvarnon/context-forge/issues/8)
* **ci:** Phase 7 — Cross-Platform CI & Release ([e9192bd](https://github.com/asvarnon/context-forge/commit/e9192bdbb6ad3debc7b59cadcd9efabb9a694c36))
* **core:** add types, traits, and error definitions for cf-core ([7b03ce5](https://github.com/asvarnon/context-forge/commit/7b03ce5e8af20635dbfd924b60f903fa42088a3e)), closes [#2](https://github.com/asvarnon/context-forge/issues/2)
* **core:** implement ContextEngine — assembly, scoring, eviction ([2647754](https://github.com/asvarnon/context-forge/commit/2647754c67a3aa212d070f79914b53647f20270c))
* **core:** implement ContextEngine — assembly, scoring, eviction ([7b6a76c](https://github.com/asvarnon/context-forge/commit/7b6a76c879bf96d41b08a787f000fb152daf0d03)), closes [#4](https://github.com/asvarnon/context-forge/issues/4)
* **core:** Phase 1 — Core crate types and traits ([2d8116d](https://github.com/asvarnon/context-forge/commit/2d8116d69da2481fa2ca470c9e329ea2d7c308c7))
* **extension:** implement Phase 6 VS Code extension integration ([4f9ba51](https://github.com/asvarnon/context-forge/commit/4f9ba51bf33a735a48e88b2c430b4ef8ea7bd7cd)), closes [#7](https://github.com/asvarnon/context-forge/issues/7)
* **extension:** Phase 6 — VS Code Extension Integration ([d9ee81d](https://github.com/asvarnon/context-forge/commit/d9ee81daa13a45343b58bd56f98b89e0b8d84b89))
* **napi:** implement Node.js bindings via napi-rs ([cbbd31f](https://github.com/asvarnon/context-forge/commit/cbbd31f039426abcb5487f31b8251e61d60471c3))
* **napi:** implement Node.js bindings via napi-rs ([8b63f16](https://github.com/asvarnon/context-forge/commit/8b63f1660ebacf5736a67f485e59ba45f919cf0e)), closes [#6](https://github.com/asvarnon/context-forge/issues/6)
* Phase 0 — Cargo workspace scaffolding ([6d6285b](https://github.com/asvarnon/context-forge/commit/6d6285b436360c6a10f309656ba3ca3f804ea55c))
* Phase 0 — Cargo workspace scaffolding ([35c1505](https://github.com/asvarnon/context-forge/commit/35c150526f0bc2883b75981b2c5e00052332b889)), closes [#1](https://github.com/asvarnon/context-forge/issues/1)
* Phase 4 — CLI Binary (PreCompact Hook Target) ([1a3d6b2](https://github.com/asvarnon/context-forge/commit/1a3d6b2d312cef585f108036be8d4fbde3182c51))
* Phase 4 CLI binary with subcommands ([673f16c](https://github.com/asvarnon/context-forge/commit/673f16c0da1c3e9efa1078831ce369003d496f8a)), closes [#5](https://github.com/asvarnon/context-forge/issues/5)
* Phase 8 — Claude Code CLI hooks integration ([96b9230](https://github.com/asvarnon/context-forge/commit/96b9230ce60d3e0ff5d8d810f5373e55ac502c43))
* Phase 8 — Claude Code CLI hooks integration ([89c4f1c](https://github.com/asvarnon/context-forge/commit/89c4f1cb458b9f8607b22fadeac806536bd852fa)), closes [#21](https://github.com/asvarnon/context-forge/issues/21)
* **storage:** implement SQLite + FTS5 storage crate ([ec42666](https://github.com/asvarnon/context-forge/commit/ec4266600930043d6d18a8c3c26068b939ad74f0))
* **storage:** implement SQLite + FTS5 storage crate ([a8180bf](https://github.com/asvarnon/context-forge/commit/a8180bf6c2b470c5403a1eb4a121182ffb7eb8e8)), closes [#3](https://github.com/asvarnon/context-forge/issues/3)


### Bug Fixes

* address PR [#13](https://github.com/asvarnon/context-forge/issues/13) review comments ([533b498](https://github.com/asvarnon/context-forge/commit/533b4980490f104fdcb6cf831c2e47a8dc03729e))
* address PR [#19](https://github.com/asvarnon/context-forge/issues/19) review comments ([2d29f01](https://github.com/asvarnon/context-forge/commit/2d29f01fa41dc258fd3f4775089304d9d74856c2))
* address PR [#20](https://github.com/asvarnon/context-forge/issues/20) review comments ([8d46c10](https://github.com/asvarnon/context-forge/commit/8d46c10d34d2dc4041c84923412a7f89833fc1e1))
* address PR review — recency-scored search_all, recv_timeout handling, test coverage ([a0282e0](https://github.com/asvarnon/context-forge/commit/a0282e0ec0b6db9a6ee04206ba48c88b23f93fff))
* **ci:** gate TypeScript steps on package-lock.json, soften dependency-review ([0e0f236](https://github.com/asvarnon/context-forge/commit/0e0f2366149d889541f3a7db6751dc42fd1ae75c))
* **ci:** remove cargo-workspace plugin from release-please ([0946250](https://github.com/asvarnon/context-forge/commit/0946250e692f65d04e65962b21cb91692651cd0a))
* **ci:** switch release-please to simple type with TOML updater ([44a5ee8](https://github.com/asvarnon/context-forge/commit/44a5ee842d4836e93cb0d3fe205c3f4cc37cb6ae))
* handle empty transcript_path, use line number in JSONL warnings ([9532ce5](https://github.com/asvarnon/context-forge/commit/9532ce52d279e90acfac7e967b0d487938dc4846))
* parse transcript_path from PreCompact metadata and read JSONL transcript ([907126c](https://github.com/asvarnon/context-forge/commit/907126cf899ad239fe2434b7d669efc86c375fbf))
* parse transcript_path from PreCompact metadata and read JSONL transcript ([c6c8f9f](https://github.com/asvarnon/context-forge/commit/c6c8f9f513214057f6854a7fe9533607adfa6c84)), closes [#30](https://github.com/asvarnon/context-forge/issues/30)
* remove deprecated --allow-proposed-api from vsce package ([f879052](https://github.com/asvarnon/context-forge/commit/f8790523e0379e1fe2943387e0d1150da3ad3bc0))
* safer default_db_path fallback chain + docs timeout example ([b4c3508](https://github.com/asvarnon/context-forge/commit/b4c35083f256058d20f1559d36e8cdc7de0c1ff5))
* **storage:** add STRICT, CHECK constraints, timestamp index, transactional save, per-conn PRAGMAs ([90d98c3](https://github.com/asvarnon/context-forge/commit/90d98c34c24d8abae51489e67a2e45a055fb3c4a))
* **storage:** address PR [#11](https://github.com/asvarnon/context-forge/issues/11) review comments ([ff8bda8](https://github.com/asvarnon/context-forge/commit/ff8bda840bf3afe24ff86b7ed55a7a6539a1d8bf))

## [0.2.0](https://github.com/asvarnon/context-forge/compare/v0.1.1...v0.2.0) (2026-04-01)


### Features

* add install scripts for all 3 platforms ([1a59d90](https://github.com/asvarnon/context-forge/commit/1a59d909cacd7dc4eb80d3ab002118384fe7bd80))
* **ci:** add napi build verification and release workflow ([0a5332a](https://github.com/asvarnon/context-forge/commit/0a5332a87bc0f5899be36397c57305b5059e10dc)), closes [#8](https://github.com/asvarnon/context-forge/issues/8)
* **ci:** Phase 7 — Cross-Platform CI & Release ([e9192bd](https://github.com/asvarnon/context-forge/commit/e9192bdbb6ad3debc7b59cadcd9efabb9a694c36))
* **core:** add types, traits, and error definitions for cf-core ([7b03ce5](https://github.com/asvarnon/context-forge/commit/7b03ce5e8af20635dbfd924b60f903fa42088a3e)), closes [#2](https://github.com/asvarnon/context-forge/issues/2)
* **core:** implement ContextEngine — assembly, scoring, eviction ([2647754](https://github.com/asvarnon/context-forge/commit/2647754c67a3aa212d070f79914b53647f20270c))
* **core:** implement ContextEngine — assembly, scoring, eviction ([7b6a76c](https://github.com/asvarnon/context-forge/commit/7b6a76c879bf96d41b08a787f000fb152daf0d03)), closes [#4](https://github.com/asvarnon/context-forge/issues/4)
* **core:** Phase 1 — Core crate types and traits ([2d8116d](https://github.com/asvarnon/context-forge/commit/2d8116d69da2481fa2ca470c9e329ea2d7c308c7))
* **extension:** implement Phase 6 VS Code extension integration ([4f9ba51](https://github.com/asvarnon/context-forge/commit/4f9ba51bf33a735a48e88b2c430b4ef8ea7bd7cd)), closes [#7](https://github.com/asvarnon/context-forge/issues/7)
* **extension:** Phase 6 — VS Code Extension Integration ([d9ee81d](https://github.com/asvarnon/context-forge/commit/d9ee81daa13a45343b58bd56f98b89e0b8d84b89))
* **napi:** implement Node.js bindings via napi-rs ([cbbd31f](https://github.com/asvarnon/context-forge/commit/cbbd31f039426abcb5487f31b8251e61d60471c3))
* **napi:** implement Node.js bindings via napi-rs ([8b63f16](https://github.com/asvarnon/context-forge/commit/8b63f1660ebacf5736a67f485e59ba45f919cf0e)), closes [#6](https://github.com/asvarnon/context-forge/issues/6)
* Phase 0 — Cargo workspace scaffolding ([6d6285b](https://github.com/asvarnon/context-forge/commit/6d6285b436360c6a10f309656ba3ca3f804ea55c))
* Phase 0 — Cargo workspace scaffolding ([35c1505](https://github.com/asvarnon/context-forge/commit/35c150526f0bc2883b75981b2c5e00052332b889)), closes [#1](https://github.com/asvarnon/context-forge/issues/1)
* Phase 4 — CLI Binary (PreCompact Hook Target) ([1a3d6b2](https://github.com/asvarnon/context-forge/commit/1a3d6b2d312cef585f108036be8d4fbde3182c51))
* Phase 4 CLI binary with subcommands ([673f16c](https://github.com/asvarnon/context-forge/commit/673f16c0da1c3e9efa1078831ce369003d496f8a)), closes [#5](https://github.com/asvarnon/context-forge/issues/5)
* Phase 8 — Claude Code CLI hooks integration ([96b9230](https://github.com/asvarnon/context-forge/commit/96b9230ce60d3e0ff5d8d810f5373e55ac502c43))
* Phase 8 — Claude Code CLI hooks integration ([89c4f1c](https://github.com/asvarnon/context-forge/commit/89c4f1cb458b9f8607b22fadeac806536bd852fa)), closes [#21](https://github.com/asvarnon/context-forge/issues/21)
* **storage:** implement SQLite + FTS5 storage crate ([ec42666](https://github.com/asvarnon/context-forge/commit/ec4266600930043d6d18a8c3c26068b939ad74f0))
* **storage:** implement SQLite + FTS5 storage crate ([a8180bf](https://github.com/asvarnon/context-forge/commit/a8180bf6c2b470c5403a1eb4a121182ffb7eb8e8)), closes [#3](https://github.com/asvarnon/context-forge/issues/3)


### Bug Fixes

* address PR [#13](https://github.com/asvarnon/context-forge/issues/13) review comments ([533b498](https://github.com/asvarnon/context-forge/commit/533b4980490f104fdcb6cf831c2e47a8dc03729e))
* address PR [#19](https://github.com/asvarnon/context-forge/issues/19) review comments ([2d29f01](https://github.com/asvarnon/context-forge/commit/2d29f01fa41dc258fd3f4775089304d9d74856c2))
* address PR [#20](https://github.com/asvarnon/context-forge/issues/20) review comments ([8d46c10](https://github.com/asvarnon/context-forge/commit/8d46c10d34d2dc4041c84923412a7f89833fc1e1))
* address PR review — recency-scored search_all, recv_timeout handling, test coverage ([a0282e0](https://github.com/asvarnon/context-forge/commit/a0282e0ec0b6db9a6ee04206ba48c88b23f93fff))
* **ci:** gate TypeScript steps on package-lock.json, soften dependency-review ([0e0f236](https://github.com/asvarnon/context-forge/commit/0e0f2366149d889541f3a7db6751dc42fd1ae75c))
* **ci:** remove cargo-workspace plugin from release-please ([0946250](https://github.com/asvarnon/context-forge/commit/0946250e692f65d04e65962b21cb91692651cd0a))
* **ci:** switch release-please to simple type with TOML updater ([44a5ee8](https://github.com/asvarnon/context-forge/commit/44a5ee842d4836e93cb0d3fe205c3f4cc37cb6ae))
* handle empty transcript_path, use line number in JSONL warnings ([9532ce5](https://github.com/asvarnon/context-forge/commit/9532ce52d279e90acfac7e967b0d487938dc4846))
* parse transcript_path from PreCompact metadata and read JSONL transcript ([907126c](https://github.com/asvarnon/context-forge/commit/907126cf899ad239fe2434b7d669efc86c375fbf))
* parse transcript_path from PreCompact metadata and read JSONL transcript ([c6c8f9f](https://github.com/asvarnon/context-forge/commit/c6c8f9f513214057f6854a7fe9533607adfa6c84)), closes [#30](https://github.com/asvarnon/context-forge/issues/30)
* remove deprecated --allow-proposed-api from vsce package ([f879052](https://github.com/asvarnon/context-forge/commit/f8790523e0379e1fe2943387e0d1150da3ad3bc0))
* safer default_db_path fallback chain + docs timeout example ([b4c3508](https://github.com/asvarnon/context-forge/commit/b4c35083f256058d20f1559d36e8cdc7de0c1ff5))
* **storage:** add STRICT, CHECK constraints, timestamp index, transactional save, per-conn PRAGMAs ([90d98c3](https://github.com/asvarnon/context-forge/commit/90d98c34c24d8abae51489e67a2e45a055fb3c4a))
* **storage:** address PR [#11](https://github.com/asvarnon/context-forge/issues/11) review comments ([ff8bda8](https://github.com/asvarnon/context-forge/commit/ff8bda840bf3afe24ff86b7ed55a7a6539a1d8bf))
