# Changelog

All notable changes to this project will be documented in this file.

## [0.3.2] - 2026-04-03

### Features

- Schema v2 migration — unified runtime adapter + importance detection foundation ([50ed978](https://github.com/asvarnon/context-forge/commit/50ed978afc6ee3883211d7088aab7f33576b90f2))
- Add analysis crate — tokenization, n-grams, and term frequency primitives ([c2eabf2](https://github.com/asvarnon/context-forge/commit/c2eabf27bb848a459750bc79069ffae9114e3c5b))
- Parse session_id and compaction_count from hook stdin ([#44](https://github.com/asvarnon/context-forge/issues/44)) ([0789ba9](https://github.com/asvarnon/context-forge/commit/0789ba90a426bd7aea2f9b4bb98cb6a1ea551a9c))
- Add fallback session grouping by timestamp proximity ([#45](https://github.com/asvarnon/context-forge/issues/45)) ([edf97ca](https://github.com/asvarnon/context-forge/commit/edf97ca0261bf4fb88184cb2032fa0b0f65543ec))
- *(analysis)* Add content pre-filtering module ([#62](https://github.com/asvarnon/context-forge/issues/62)) ([d7ba9c9](https://github.com/asvarnon/context-forge/commit/d7ba9c991419124c7445b08227a8aa4524287bfd))
- Implement session frequency and recurrence scoring ([#47](https://github.com/asvarnon/context-forge/issues/47)) ([e5c916e](https://github.com/asvarnon/context-forge/commit/e5c916e69c366e2eb1b589c49a8bcbff2341a7dd))

### Bug Fixes

- Address Copilot review comments on analysis crate ([86e5631](https://github.com/asvarnon/context-forge/commit/86e5631b3486a97f3ff2c181303f2e0d6f784b52))
- Use char-safe truncation for session_id in error message ([9c9bd29](https://github.com/asvarnon/context-forge/commit/9c9bd291d0e3505bfb0144db212e37796b01353b))
- Address PR #61 review comments ([a60e488](https://github.com/asvarnon/context-forge/commit/a60e48812d051bf68b88da4fad418646f2caf98d))
- *(analysis)* Address PR #63 Copilot review comments ([5840135](https://github.com/asvarnon/context-forge/commit/584013578591bb76e2f9c1cca48006c83aac1ecb))

### Refactoring

- Address PR #64 review comments ([79dc054](https://github.com/asvarnon/context-forge/commit/79dc0540e4d59b161d2aa7274c0a1bbddd399a4a))

## [0.3.1] - 2026-04-02

### Bug Fixes

- Sanitize FTS5 operator chars in query input ([8e0ed1a](https://github.com/asvarnon/context-forge/commit/8e0ed1acbe94d0a79eae696ca35bb57926d5e6d1))
- *(ci)* Guard head_commit.message with event_name check ([436b6bd](https://github.com/asvarnon/context-forge/commit/436b6bd0178c24edbd5e6e944c5deeb12bb9a151))
- *(ci)* Quote YAML if-expressions to prevent parse errors ([e913f9f](https://github.com/asvarnon/context-forge/commit/e913f9fa732d72dcb90d9d1d56fb5e4c234788ec))

### Miscellaneous

- Extract reusable CI workflow, skip redundant runs on release ([69b31be](https://github.com/asvarnon/context-forge/commit/69b31bef1fa73bab0681448307c5db1d3fb5264a))
- Release v0.3.1 ([e3cbd3c](https://github.com/asvarnon/context-forge/commit/e3cbd3c643dc7628cc4d569a6eb39cbda5c96662))

## [0.3.0] - 2026-04-02

### Features

- Increase default token budget to 16,000 ([8720f6a](https://github.com/asvarnon/context-forge/commit/8720f6a372c7c87fbf16bba235eb2454d21a5e40))
- Custom search queries, config file, FTS5 preprocessing, tunable half-life ([#34](https://github.com/asvarnon/context-forge/issues/34)) ([234c42b](https://github.com/asvarnon/context-forge/commit/234c42bb60659090677d4c62c7ad96b4acc5e2b3))

### Bug Fixes

- Address PR review comments ([#38](https://github.com/asvarnon/context-forge/issues/38)) ([c5b3e85](https://github.com/asvarnon/context-forge/commit/c5b3e8524c3548d619528dba41a8dfb344d59653))

### Documentation

- Add Clean Code and Research agents to architecture docs ([6371ef2](https://github.com/asvarnon/context-forge/commit/6371ef2ed7d6b788c982c2a2a74234fe906d9d21))

### Styling

- Fix cargo fmt formatting in engine tests ([cddd577](https://github.com/asvarnon/context-forge/commit/cddd577a6766cf0ef126aa4a681f108b535d97af))

### Miscellaneous

- Add Clean Code and Research agents ([3745b8f](https://github.com/asvarnon/context-forge/commit/3745b8f41b81a9c8b23ab3e4e044ec1464c1befc))
- Enhance Clean Code and Research agents ([08fbd39](https://github.com/asvarnon/context-forge/commit/08fbd39ce95eef215e49aff741ce001cf8b2ac04))
- Release v0.3.0 ([0e8b6d3](https://github.com/asvarnon/context-forge/commit/0e8b6d34db0dcaf88017e031ea35a461c7ff75f9))

## [0.2.2] - 2026-04-01

### Bug Fixes

- *(ci)* Harden release pipeline and install scripts ([08ddeb4](https://github.com/asvarnon/context-forge/commit/08ddeb4ff2d17a9fb93b2026694a9f70a63c083c))

### Miscellaneous

- Release v0.2.2 ([007879b](https://github.com/asvarnon/context-forge/commit/007879b088a2b3f4988c16c9fc8a61f33a883763))

## [0.2.1] - 2026-04-01

### Bug Fixes

- Use workspace_root in pre-release-hook to prevent per-crate changelogs ([7d56205](https://github.com/asvarnon/context-forge/commit/7d56205f00de4f7ff1638e579978d1a774e28c5e))
- Move pre-release-hook to cli crate only (runs once, not per-crate) ([349a816](https://github.com/asvarnon/context-forge/commit/349a816f7a7a2609de679ccaea785a0bd0d07088))

### Miscellaneous

- Sync Cargo.lock with v0.2.0 ([fec9f4d](https://github.com/asvarnon/context-forge/commit/fec9f4d72ca7e0e6ef05a5489ff31a73974c6d20))
- Switch from release-please to cargo-release + git-cliff ([4d629f1](https://github.com/asvarnon/context-forge/commit/4d629f10346fd9b095b1e0028083f3ce70545a6e))
- Release v0.2.1 ([bfd20bd](https://github.com/asvarnon/context-forge/commit/bfd20bd408dbc49cf84863df79a26d06bf23d606))

## [0.2.0] - 2026-04-01

### Bug Fixes

- Parse transcript_path from PreCompact metadata and read JSONL transcript ([c6c8f9f](https://github.com/asvarnon/context-forge/commit/c6c8f9f513214057f6854a7fe9533607adfa6c84))
- Handle empty transcript_path, use line number in JSONL warnings ([9532ce5](https://github.com/asvarnon/context-forge/commit/9532ce52d279e90acfac7e967b0d487938dc4846))
- *(ci)* Remove cargo-workspace plugin from release-please ([0946250](https://github.com/asvarnon/context-forge/commit/0946250e692f65d04e65962b21cb91692651cd0a))
- *(ci)* Switch release-please to simple type with TOML updater ([44a5ee8](https://github.com/asvarnon/context-forge/commit/44a5ee842d4836e93cb0d3fe205c3f4cc37cb6ae))

### Documentation

- Update README for v0.1.0, add ARCHITECTURE.md developer docs ([e146780](https://github.com/asvarnon/context-forge/commit/e146780a1d2cc0ed93b48f43d9e3cc6dce954091))

### Miscellaneous

- Bump workspace version to 0.1.1 ([3b0449f](https://github.com/asvarnon/context-forge/commit/3b0449f09cab76acd70cf2802e2198bb3541f375))
- Add release-please for automated version management ([ed0071a](https://github.com/asvarnon/context-forge/commit/ed0071aa109fc94cbb6c537262e75d7ed08ee116))
- Retrigger release-please after enabling PR permissions ([007fa6b](https://github.com/asvarnon/context-forge/commit/007fa6b3ccd61475108ed4c32be8e1c91dd2cd31))
- *(main)* Release 0.2.0 ([ed2ff62](https://github.com/asvarnon/context-forge/commit/ed2ff62c4257d28028f9614b82ecb340d372f2c1))

## [0.1.0] - 2026-04-01

### Features

- Phase 0 — Cargo workspace scaffolding ([35c1505](https://github.com/asvarnon/context-forge/commit/35c150526f0bc2883b75981b2c5e00052332b889))
- *(core)* Add types, traits, and error definitions for cf-core ([7b03ce5](https://github.com/asvarnon/context-forge/commit/7b03ce5e8af20635dbfd924b60f903fa42088a3e))
- *(storage)* Implement SQLite + FTS5 storage crate ([a8180bf](https://github.com/asvarnon/context-forge/commit/a8180bf6c2b470c5403a1eb4a121182ffb7eb8e8))
- *(core)* Implement ContextEngine — assembly, scoring, eviction ([7b6a76c](https://github.com/asvarnon/context-forge/commit/7b6a76c879bf96d41b08a787f000fb152daf0d03))
- Phase 4 CLI binary with subcommands ([673f16c](https://github.com/asvarnon/context-forge/commit/673f16c0da1c3e9efa1078831ce369003d496f8a))
- *(napi)* Implement Node.js bindings via napi-rs ([8b63f16](https://github.com/asvarnon/context-forge/commit/8b63f1660ebacf5736a67f485e59ba45f919cf0e))
- *(extension)* Implement Phase 6 VS Code extension integration ([4f9ba51](https://github.com/asvarnon/context-forge/commit/4f9ba51bf33a735a48e88b2c430b4ef8ea7bd7cd))
- *(ci)* Add napi build verification and release workflow ([0a5332a](https://github.com/asvarnon/context-forge/commit/0a5332a87bc0f5899be36397c57305b5059e10dc))
- Phase 8 — Claude Code CLI hooks integration ([89c4f1c](https://github.com/asvarnon/context-forge/commit/89c4f1cb458b9f8607b22fadeac806536bd852fa))
- Add install scripts for all 3 platforms ([1a59d90](https://github.com/asvarnon/context-forge/commit/1a59d909cacd7dc4eb80d3ab002118384fe7bd80))

### Bug Fixes

- *(ci)* Gate TypeScript steps on package-lock.json, soften dependency-review ([0e0f236](https://github.com/asvarnon/context-forge/commit/0e0f2366149d889541f3a7db6751dc42fd1ae75c))
- *(storage)* Add STRICT, CHECK constraints, timestamp index, transactional save, per-conn PRAGMAs ([90d98c3](https://github.com/asvarnon/context-forge/commit/90d98c34c24d8abae51489e67a2e45a055fb3c4a))
- *(storage)* Address PR #11 review comments ([ff8bda8](https://github.com/asvarnon/context-forge/commit/ff8bda840bf3afe24ff86b7ed55a7a6539a1d8bf))
- Address PR #13 review comments ([533b498](https://github.com/asvarnon/context-forge/commit/533b4980490f104fdcb6cf831c2e47a8dc03729e))
- Address PR review — recency-scored search_all, recv_timeout handling, test coverage ([a0282e0](https://github.com/asvarnon/context-forge/commit/a0282e0ec0b6db9a6ee04206ba48c88b23f93fff))
- Address PR #19 review comments ([2d29f01](https://github.com/asvarnon/context-forge/commit/2d29f01fa41dc258fd3f4775089304d9d74856c2))
- Address PR #20 review comments ([8d46c10](https://github.com/asvarnon/context-forge/commit/8d46c10d34d2dc4041c84923412a7f89833fc1e1))
- Safer default_db_path fallback chain + docs timeout example ([b4c3508](https://github.com/asvarnon/context-forge/commit/b4c35083f256058d20f1559d36e8cdc7de0c1ff5))
- Remove deprecated --allow-proposed-api from vsce package ([f879052](https://github.com/asvarnon/context-forge/commit/f8790523e0379e1fe2943387e0d1150da3ad3bc0))

### Refactoring

- *(core)* Replace source: String with kind: EntryKind, standardize token_count to usize ([c9b7756](https://github.com/asvarnon/context-forge/commit/c9b775680a5c92f2f49aecff08446d1354abdacc))
- *(napi)* Make close() async via CloseTask ([b0219a3](https://github.com/asvarnon/context-forge/commit/b0219a36086757f6b477efd4d17d21f69efc2598))

### Documentation

- Add project README and CONTRIBUTING guide ([efcfe10](https://github.com/asvarnon/context-forge/commit/efcfe101bf87e77100aa454e7bb4c506b1ce31d6))

### Styling

- Cargo fmt formatting fixes ([aeea306](https://github.com/asvarnon/context-forge/commit/aeea3069e78daa13724f4fbb158add75c2e0e94e))
- Cargo fmt ([fb2680d](https://github.com/asvarnon/context-forge/commit/fb2680d141240af5d13a4807fc004592f7eba642))

### Miscellaneous

- Add agent suite and design principles ([9992384](https://github.com/asvarnon/context-forge/commit/9992384270e7ab94617c2d4f29ea9728fb1abb93))
- Add GitHub Actions workflows (CI, audit, dependency-review) ([6e6736b](https://github.com/asvarnon/context-forge/commit/6e6736b2a6166032c43c93e43adc18a1a9b03bfb))
- Remove dependency-review (unsupported on free private repos) ([f19235d](https://github.com/asvarnon/context-forge/commit/f19235dbfbcf17d1a96536c3cac7bb8e85746d34))
- Add .vscodeignore and repository field to extension ([9ecbf8c](https://github.com/asvarnon/context-forge/commit/9ecbf8cc40041376cc6b436a716f9372a0c702e4))


