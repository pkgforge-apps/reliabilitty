
## [0.1.19](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.18...v0.1.19) - 2026-11-23

### Bug Fixes

- Address resize review findings ([#45](https://github.com/pkgforge-apps/reliabilitty/pull/45)) - ([d867e80](https://github.com/pkgforge-apps/reliabilitty/commit/d867e803a531462b54c8dd2deef9ac5863b20c3a))

### Performance

- Optimize event loop, rendering, VT parsing and deduplicate code ([#46](https://github.com/pkgforge-apps/reliabilitty/pull/46)) - ([c78a8e0](https://github.com/pkgforge-apps/reliabilitty/commit/c78a8e01780b1d51873c119f665fba18face784a))
- Keep interactive resizes smooth without flicker ([#43](https://github.com/pkgforge-apps/reliabilitty/pull/43)) - ([870ceb6](https://github.com/pkgforge-apps/reliabilitty/commit/870ceb6e0c158e8de6ca90d72741a3d00f62b9f0))

## [0.1.18](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.17...v0.1.18) - 2026-11-21

### Documentation

- Refresh benchmarks for v0.1.17 ([#40](https://github.com/pkgforge-apps/reliabilitty/pull/40)) - ([40b3b58](https://github.com/pkgforge-apps/reliabilitty/commit/40b3b5880341d6c0a7e0c2da3ad91043dd2511ef))

### Performance

- Defer scrollback reflow until resize settles ([#42](https://github.com/pkgforge-apps/reliabilitty/pull/42)) - ([2419e86](https://github.com/pkgforge-apps/reliabilitty/commit/2419e86b37aa896ad80d81c6996697c37851a1f6))

## [0.1.17](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.16...v0.1.17) - 2026-11-21

### Features

- Support OSC 8 hyperlinks with ctrl+click ([#39](https://github.com/pkgforge-apps/reliabilitty/pull/39)) - ([1fd9e2a](https://github.com/pkgforge-apps/reliabilitty/commit/1fd9e2ae12089f5fe56e19845e3278874ffb8240))

### Performance

- Derive frame interval from display refresh rate ([#36](https://github.com/pkgforge-apps/reliabilitty/pull/36)) - ([7cf7aba](https://github.com/pkgforge-apps/reliabilitty/commit/7cf7aba9d94ffe700c5465d657a49addee532a59))

## [0.1.16](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.15...v0.1.16) - 2026-11-20

### Performance

- Fast-path indexed SGR and shorten frame interval ([#34](https://github.com/pkgforge-apps/reliabilitty/pull/34)) - ([758c1bb](https://github.com/pkgforge-apps/reliabilitty/commit/758c1bba608b688f7845ac840b3b1d4f6a760af2))

## [0.1.15](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.14...v0.1.15) - 2026-11-20

### Documentation

- Add DOOM-fire frame rate benchmark ([#31](https://github.com/pkgforge-apps/reliabilitty/pull/31)) - ([cd07bc8](https://github.com/pkgforge-apps/reliabilitty/commit/cd07bc8204fef0a35882542a544be8942b78145e))
- Refresh benchmarks for v0.1.14 with perf CPU measurements ([#29](https://github.com/pkgforge-apps/reliabilitty/pull/29)) - ([1a2eecf](https://github.com/pkgforge-apps/reliabilitty/commit/1a2eecf5c35e3c0289987a29d7b237fff1a20759))

### Performance

- Decouple PTY reads from rendering ([#33](https://github.com/pkgforge-apps/reliabilitty/pull/33)) - ([dcb68f5](https://github.com/pkgforge-apps/reliabilitty/commit/dcb68f50922f344cb4cb61cef81b816e562d97d6))

## [0.1.14](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.13...v0.1.14) - 2026-11-20

### Performance

- Restore mimalloc as the global allocator ([#27](https://github.com/pkgforge-apps/reliabilitty/pull/27)) - ([d9ac555](https://github.com/pkgforge-apps/reliabilitty/commit/d9ac555373dea203238f6f590ef6776f7affcb69))

## [0.1.13](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.12...v0.1.13) - 2026-11-20

### Documentation

- Refresh benchmarks with foot on v0.1.12 ([#24](https://github.com/pkgforge-apps/reliabilitty/pull/24)) - ([b94fb46](https://github.com/pkgforge-apps/reliabilitty/commit/b94fb466b3b914add381cb38fe01ea3ea14cbbb9))

### Performance

- Faster non-ASCII and SGR parsing, cached font database, drop mimalloc ([#26](https://github.com/pkgforge-apps/reliabilitty/pull/26)) - ([e4656f0](https://github.com/pkgforge-apps/reliabilitty/commit/e4656f0242573c99dd00dd0f237a205d4ff1e6e2))

## [0.1.12](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.11...v0.1.12) - 2026-11-19

### Bug Fixes

- Compile on stable and restrict print fast path - ([dde4c46](https://github.com/pkgforge-apps/reliabilitty/commit/dde4c46105135b432cf1474286d1a1540170fba9))

### Documentation

- Benchmark v0.1.11 across six terminals ([#22](https://github.com/pkgforge-apps/reliabilitty/pull/22)) - ([3a0209f](https://github.com/pkgforge-apps/reliabilitty/commit/3a0209fde11c70c861a26f339a6a5926e45ad990))

### Performance

- Fast-path ASCII printing, single-line scrolling and blank cells - ([7734517](https://github.com/pkgforge-apps/reliabilitty/commit/77345175f49f8afee50c4977d35aece7eab9e3a5))

## [0.1.11](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.10...v0.1.11) - 2026-11-19

### Features

- Support the kitty keyboard protocol disambiguation flag - ([17d2442](https://github.com/pkgforge-apps/reliabilitty/commit/17d2442f778e65d0305fd2262c8da074cdcf9727))

### Documentation

- Add terminals to feature table and sort tables alphabetically - ([5f5eb20](https://github.com/pkgforge-apps/reliabilitty/commit/5f5eb203a9303280eb7f0e37e98cbc742382c7fc))

### Performance

- *(vt)* Fast-path CSI cursor positioning sequences - ([e16d6a6](https://github.com/pkgforge-apps/reliabilitty/commit/e16d6a6dd4649bb02dd4167131d94a796494b2f4))
- *(vt)* Fast-path UTF-8 decoding and widen font fallbacks - ([e6a93cf](https://github.com/pkgforge-apps/reliabilitty/commit/e6a93cfbdeaa13a6fe652bc91f3f9d2a2bba8f21))

## [0.1.10](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.9...v0.1.10) - 2026-11-18

### Performance

- *(vt)* Blank scrolled lines in place without a pool - ([bd5c92e](https://github.com/pkgforge-apps/reliabilitty/commit/bd5c92ea3b2342eaa12a81b9fced228dfe6805f1))
- *(vt)* Make scroll regions O(1) with a logical line ring - ([1f1ca72](https://github.com/pkgforge-apps/reliabilitty/commit/1f1ca726a0c49f06ae2d796872fa2365e36a4f89))

## [0.1.9](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.8...v0.1.9) - 2026-11-18

### Features

- Add context menu and scrollback search ([#19](https://github.com/pkgforge-apps/reliabilitty/pull/19)) - ([a1e540b](https://github.com/pkgforge-apps/reliabilitty/commit/a1e540b9e61430781075683b4a2f6b3c5a66d798))

### Documentation

- Split search and Vi mode into separate feature rows - ([09bfbf7](https://github.com/pkgforge-apps/reliabilitty/commit/09bfbf71e130eb687017bfa6da919da22ee01d70))
- Refresh benchmarks with six terminals on v0.1.8 ([#10](https://github.com/pkgforge-apps/reliabilitty/pull/10)) - ([879b392](https://github.com/pkgforge-apps/reliabilitty/commit/879b392c0f767fcc51bc4f2a39003530c2ac5ba4))

## [0.1.8](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.7...v0.1.8) - 2026-11-18

### Performance

- *(vt)* Narrow region-scroll damage and rotate lines in place - ([602b167](https://github.com/pkgforge-apps/reliabilitty/commit/602b1679cd657ccd0d8cd9464f87e24c3af13c90))

## [0.1.7](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.6...v0.1.7) - 2026-11-18

### Bug Fixes

- Satisfy CI clippy lints in deque scroll code - ([1d6a701](https://github.com/pkgforge-apps/reliabilitty/commit/1d6a701c003ad8b34ebacb1da4f648968aa5c17d))

## [0.1.6](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.5...v0.1.6) - 2026-11-18

### Performance

- *(vt)* Clear recycled lines lazily and avoid CSI allocations - ([dc961e6](https://github.com/pkgforge-apps/reliabilitty/commit/dc961e667be8d9435ffe0dd89ac7e8e66ffabfa1))

## [0.1.5](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.4...v0.1.5) - 2026-11-18

### Performance

- *(vt)* Use a deque for the screen buffer so full scrolls are O(1) - ([3cef596](https://github.com/pkgforge-apps/reliabilitty/commit/3cef596862c62fb5a61c2a9ebccd999d6aa74dcb))
- Drain PTY before drawing and speed up text and glyph handling - ([d4b0e71](https://github.com/pkgforge-apps/reliabilitty/commit/d4b0e719d933ed147767c2ed73a6e343eee7cc5c))

## [0.1.4](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.3...v0.1.4) - 2026-11-18

### Bug Fixes

- Recognize numpad keys and keep pager scroll content - ([21c7bd2](https://github.com/pkgforge-apps/reliabilitty/commit/21c7bd2bba57728ebff846ebb404618e1ab82c48))

### Documentation

- Update performance results ([#8](https://github.com/pkgforge-apps/reliabilitty/pull/8)) - ([af38a63](https://github.com/pkgforge-apps/reliabilitty/commit/af38a630f24dccb44f72433ceae610e102719929))

## [0.1.3](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.2...v0.1.3) - 2026-11-17

### Performance

- Use mimalloc as the global allocator - ([15d2e3f](https://github.com/pkgforge-apps/reliabilitty/commit/15d2e3ffdd50e891502571da31ee9ec057aa650b))
- Optimize scrolling and text rendering - ([ec90d7b](https://github.com/pkgforge-apps/reliabilitty/commit/ec90d7b31d72a718b87e11eb33c2bd6a5b8a5ae0))

### Miscellaneous Tasks

- Build musl targets with cross for mimalloc - ([b2c9c04](https://github.com/pkgforge-apps/reliabilitty/commit/b2c9c049b79c9b1f711976214860358a6c00f550))

## [0.1.2](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.1...v0.1.2) - 2026-11-17

### Features

- Detect display DPI from X11 properties ([#5](https://github.com/pkgforge-apps/reliabilitty/pull/5)) - ([26281c9](https://github.com/pkgforge-apps/reliabilitty/commit/26281c9978602628ef19f3216d836942ae0216f1))

## [0.1.1](https://github.com/pkgforge-apps/reliabilitty/compare/v0.1.0...v0.1.1) - 2026-11-17

### Documentation

- Fix and extend the feature comparison table - ([de9cff7](https://github.com/pkgforge-apps/reliabilitty/commit/de9cff7269253435e275399b2dabe3fbb8aecd8a))

### Performance

- Draw the window before loading fonts - ([fed95c1](https://github.com/pkgforge-apps/reliabilitty/commit/fed95c103ae5e3cda781f5127a31378fa104015d))
