# Windows Release 第三方运行依赖许可证清单

本文件由锁定的 `Cargo.lock` 和 Windows x64 主程序运行依赖图生成，用于补充
[第三方来源与许可证声明](THIRD-PARTY-NOTICES.md)。项目自身的许可证仍为
[AGPL-3.0-only](LICENSE)。

生成命令：

```powershell
cargo tree -p repl-app --target x86_64-pc-windows-msvc --edges normal --prefix none --no-dedupe --format '{p}|{l}|{r}'
```

生成日期：2026-08-10。去重后共 **342** 个 crate+版本记录；不包含本工作区 crate、仅测试依赖和仅构建期依赖。

许可证表达式来自各 crate 发布元数据。`OR` 表示上游允许选择其中一种许可证；本项目对 Slint
明确选择 `GPL-3.0-only`，对 `ort` / `ort-sys` 选择 MIT。发布二进制时应同时提供本清单、
`THIRD-PARTY-NOTICES.md` 及相应上游许可证全文。

## 许可证表达式汇总

| 许可证表达式 | crate+版本数量 |
| --- | ---: |
| `(MIT OR Apache-2.0) AND Unicode-3.0` | 1 |
| `0BSD OR MIT OR Apache-2.0` | 1 |
| `Apache-2.0` | 3 |
| `Apache-2.0 AND MIT` | 1 |
| `Apache-2.0 OR MIT` | 27 |
| `Apache-2.0/MIT` | 3 |
| `BSD-2-Clause` | 4 |
| `BSD-2-Clause OR Apache-2.0 OR MIT` | 2 |
| `BSD-3-Clause` | 6 |
| `BSD-3-Clause OR Apache-2.0` | 2 |
| `BSD-3-Clause OR MIT OR Apache-2.0` | 2 |
| `BSL-1.0` | 2 |
| `CC0-1.0 OR Apache-2.0` | 1 |
| `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | 9 |
| `MIT` | 53 |
| `MIT / Apache-2.0` | 1 |
| `MIT OR Apache-2.0` | 169 |
| `MIT OR Apache-2.0 OR Zlib` | 6 |
| `MIT OR Zlib OR Apache-2.0` | 1 |
| `MIT/Apache-2.0` | 12 |
| `Unicode-3.0` | 25 |
| `Unlicense OR MIT` | 6 |
| `Zlib` | 2 |
| `Zlib OR Apache-2.0 OR MIT` | 3 |

## 依赖明细

| Crate | 版本 | 许可证 | 上游 |
| --- | --- | --- | --- |
| `adler2` | `2.0.1` | `0BSD OR MIT OR Apache-2.0` | [源码](https://github.com/oyvindln/adler2) |
| `aho-corasick` | `1.1.4` | `Unlicense OR MIT` | [源码](https://github.com/BurntSushi/aho-corasick) |
| `aligned` | `0.4.3` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-embedded-community/aligned) |
| `aligned-vec` | `0.6.4` | `MIT` | [源码](https://github.com/sarah-ek/aligned-vec/) |
| `allocator-api2` | `0.2.21` | `MIT OR Apache-2.0` | [源码](https://github.com/zakarumych/allocator-api2) |
| `annotate-snippets` | `0.12.16` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/annotate-snippets-rs) |
| `anstream` | `1.0.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-cli/anstyle.git) |
| `anstyle` | `1.0.14` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-cli/anstyle.git) |
| `anstyle-parse` | `1.0.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-cli/anstyle.git) |
| `anstyle-query` | `1.1.5` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-cli/anstyle.git) |
| `anstyle-wincon` | `3.0.11` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-cli/anstyle.git) |
| `anyhow` | `1.0.104` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/anyhow) |
| `arg_enum_proc_macro` | `0.3.4` | `MIT` | [源码](https://github.com/lu-zero/arg_enum_proc_macro) |
| `arrayref` | `0.3.9` | `BSD-2-Clause` | [源码](https://github.com/droundy/arrayref) |
| `arrayvec` | `0.7.8` | `MIT OR Apache-2.0` | [源码](https://github.com/bluss/arrayvec) |
| `as-slice` | `0.2.1` | `MIT OR Apache-2.0` | [源码](https://github.com/japaric/as-slice) |
| `auto_enums` | `0.8.10` | `Apache-2.0 OR MIT` | [源码](https://github.com/taiki-e/auto_enums) |
| `av-scenechange` | `0.14.1` | `MIT` | [源码](https://github.com/rust-av/av-scenechange) |
| `av1-grain` | `0.2.5` | `BSD-2-Clause` | [源码](https://github.com/rust-av/av1-grain) |
| `avif-serialize` | `0.8.9` | `BSD-3-Clause` | [源码](https://github.com/kornelski/avif-serialize) |
| `base64` | `0.22.1` | `MIT OR Apache-2.0` | [源码](https://github.com/marshallpierce/rust-base64) |
| `bit_field` | `0.10.3` | `Apache-2.0/MIT` | [源码](https://github.com/phil-opp/rust-bit-field) |
| `bitflags` | `2.13.1` | `MIT OR Apache-2.0` | [源码](https://github.com/bitflags/bitflags) |
| `bitstream-io` | `4.10.0` | `MIT/Apache-2.0` | [源码](https://github.com/tuffy/bitstream-io) |
| `block-buffer` | `0.10.4` | `MIT OR Apache-2.0` | [源码](https://github.com/RustCrypto/utils) |
| `by_address` | `1.2.1` | `MIT OR Apache-2.0` | [源码](https://github.com/mbrubeck/by_address) |
| `bytemuck` | `1.25.2` | `Zlib OR Apache-2.0 OR MIT` | [源码](https://github.com/Lokathor/bytemuck) |
| `bytemuck_derive` | `1.11.0` | `Zlib OR Apache-2.0 OR MIT` | [源码](https://github.com/Lokathor/bytemuck) |
| `byteorder` | `1.5.0` | `Unlicense OR MIT` | [源码](https://github.com/BurntSushi/byteorder) |
| `byteorder-lite` | `0.1.0` | `Unlicense OR MIT` | [源码](https://github.com/image-rs/byteorder-lite) |
| `bytes` | `1.12.1` | `MIT` | [源码](https://github.com/tokio-rs/bytes) |
| `cfg-if` | `1.0.4` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/cfg-if) |
| `chrono` | `0.4.45` | `MIT OR Apache-2.0` | [源码](https://github.com/chronotope/chrono) |
| `clipboard-win` | `5.4.1` | `BSL-1.0` | [源码](https://github.com/DoumanAsh/clipboard-win) |
| `clru` | `0.6.3` | `MIT` | [源码](https://github.com/marmeladema/clru-rs) |
| `color_quant` | `1.1.0` | `MIT` | [源码](https://github.com/image-rs/color_quant.git) |
| `colorchoice` | `1.0.5` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-cli/anstyle.git) |
| `const-field-offset` | `0.2.0` | `MIT OR Apache-2.0` | [源码](https://github.com/slint-ui/slint) |
| `const-field-offset-macro` | `0.2.0` | `MIT OR Apache-2.0` | [源码](https://github.com/slint-ui/slint) |
| `convert_case` | `0.10.0` | `MIT` | [源码](https://github.com/rutrum/convert-case) |
| `copypasta` | `0.10.2` | `MIT / Apache-2.0` | [源码](https://github.com/alacritty/copypasta) |
| `core_maths` | `0.1.1` | `MIT` | [源码](https://github.com/robertbastian/core_maths) |
| `countme` | `3.0.1` | `MIT OR Apache-2.0` | [源码](https://github.com/matklad/countme) |
| `cpufeatures` | `0.2.17` | `MIT OR Apache-2.0` | [源码](https://github.com/RustCrypto/utils) |
| `crc32fast` | `1.5.0` | `MIT OR Apache-2.0` | [源码](https://github.com/srijs/rust-crc32fast) |
| `critical-section` | `1.2.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-embedded/critical-section) |
| `crossbeam-channel` | `0.5.16` | `MIT OR Apache-2.0` | [源码](https://github.com/crossbeam-rs/crossbeam) |
| `crossbeam-deque` | `0.8.7` | `MIT OR Apache-2.0` | [源码](https://github.com/crossbeam-rs/crossbeam) |
| `crossbeam-epoch` | `0.9.20` | `MIT OR Apache-2.0` | [源码](https://github.com/crossbeam-rs/crossbeam) |
| `crossbeam-utils` | `0.8.22` | `MIT OR Apache-2.0` | [源码](https://github.com/crossbeam-rs/crossbeam) |
| `crypto-common` | `0.1.7` | `MIT OR Apache-2.0` | [源码](https://github.com/RustCrypto/traits) |
| `cursor-icon` | `1.2.0` | `MIT OR Apache-2.0 OR Zlib` | [源码](https://github.com/rust-windowing/cursor-icon) |
| `data-encoding` | `2.11.0` | `MIT` | [源码](https://github.com/ia0/data-encoding) |
| `data-url` | `0.3.2` | `MIT OR Apache-2.0` | [源码](https://github.com/servo/rust-url) |
| `derive_more` | `2.1.1` | `MIT` | [源码](https://github.com/JelteF/derive_more) |
| `derive_more-impl` | `2.1.1` | `MIT` | [源码](https://github.com/JelteF/derive_more) |
| `derive_utils` | `0.16.0` | `Apache-2.0 OR MIT` | [源码](https://github.com/taiki-e/derive_utils) |
| `digest` | `0.10.7` | `MIT OR Apache-2.0` | [源码](https://github.com/RustCrypto/traits) |
| `displaydoc` | `0.2.6` | `MIT OR Apache-2.0` | [源码](https://github.com/yaahc/displaydoc) |
| `dpi` | `0.1.2` | `Apache-2.0 AND MIT` | [源码](https://github.com/rust-windowing/winit) |
| `either` | `1.17.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rayon-rs/either) |
| `env_filter` | `2.0.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-cli/env_logger) |
| `env_logger` | `0.11.11` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-cli/env_logger) |
| `equator` | `0.4.2` | `MIT` | [源码](https://github.com/sarah-ek/equator/) |
| `equator-macro` | `0.4.2` | `MIT` | [源码](https://github.com/sarah-ek/equator/) |
| `equivalent` | `1.0.2` | `Apache-2.0 OR MIT` | [源码](https://github.com/indexmap-rs/equivalent) |
| `error-code` | `3.3.2` | `BSL-1.0` | [源码](https://github.com/DoumanAsh/error-code) |
| `euclid` | `0.22.14` | `MIT OR Apache-2.0` | [源码](https://github.com/servo/euclid) |
| `exr` | `1.74.2` | `BSD-3-Clause` | [源码](https://github.com/johannesvollmer/exrs) |
| `fax` | `0.2.7` | `MIT` | [源码](https://github.com/pdf-rs/fax) |
| `fdeflate` | `0.3.7` | `MIT OR Apache-2.0` | [源码](https://github.com/image-rs/fdeflate) |
| `field-offset` | `0.3.6` | `MIT OR Apache-2.0` | [源码](https://github.com/Diggsey/rust-field-offset) |
| `fixed_decimal` | `0.7.2` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `flate2` | `1.1.9` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/flate2-rs) |
| `float-cmp` | `0.9.0` | `MIT` | [源码](https://github.com/mikedilger/float-cmp) |
| `foldhash` | `0.2.0` | `Zlib` | [源码](https://github.com/orlp/foldhash) |
| `font-types` | `0.11.3` | `MIT OR Apache-2.0` | [源码](https://github.com/googlefonts/fontations) |
| `font-types` | `0.12.2` | `MIT OR Apache-2.0` | [源码](https://github.com/googlefonts/fontations) |
| `fontdb` | `0.23.0` | `MIT` | [源码](https://github.com/RazrFalcon/fontdb) |
| `fontique` | `0.10.0` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/parley) |
| `form_urlencoded` | `1.2.2` | `MIT OR Apache-2.0` | [源码](https://github.com/servo/rust-url) |
| `generic-array` | `0.14.7` | `MIT` | [源码](https://github.com/fizyk20/generic-array.git) |
| `getopts` | `0.2.24` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/getopts) |
| `getrandom` | `0.2.17` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-random/getrandom) |
| `gif` | `0.14.2` | `MIT OR Apache-2.0` | [源码](https://github.com/image-rs/image-gif) |
| `half` | `2.7.1` | `MIT OR Apache-2.0` | [源码](https://github.com/VoidStarKat/half-rs) |
| `harfrust` | `0.8.4` | `MIT` | [源码](https://github.com/harfbuzz/harfrust) |
| `hashbrown` | `0.14.5` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/hashbrown) |
| `hashbrown` | `0.16.1` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/hashbrown) |
| `hashbrown` | `0.17.1` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/hashbrown) |
| `heck` | `0.5.0` | `MIT OR Apache-2.0` | [源码](https://github.com/withoutboats/heck) |
| `htmlparser` | `0.2.1` | `MIT OR Apache-2.0` | [源码](https://github.com/jdrouet/htmlparser.git) |
| `http` | `1.4.2` | `MIT OR Apache-2.0` | [源码](https://github.com/hyperium/http) |
| `httparse` | `1.10.1` | `MIT OR Apache-2.0` | [源码](https://github.com/seanmonstar/httparse) |
| `i-slint-backend-selector` | `1.17.1` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | [源码](https://github.com/slint-ui/slint) |
| `i-slint-backend-winit` | `1.17.1` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | [源码](https://github.com/slint-ui/slint) |
| `i-slint-common` | `1.17.1` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | [源码](https://github.com/slint-ui/slint) |
| `i-slint-compiler` | `1.17.1` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | [源码](https://github.com/slint-ui/slint) |
| `i-slint-core` | `1.17.1` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | [源码](https://github.com/slint-ui/slint) |
| `i-slint-core-macros` | `1.17.1` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | [源码](https://github.com/slint-ui/slint) |
| `i-slint-renderer-software` | `1.17.1` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | [源码](https://github.com/slint-ui/slint) |
| `icu_collections` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_decimal` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_decimal_data` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_locale` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_locale_core` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_locale_data` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_normalizer` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_normalizer_data` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_properties` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_properties_data` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_provider` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_segmenter` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `icu_segmenter_data` | `2.2.0` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `idna` | `1.1.0` | `MIT OR Apache-2.0` | [源码](https://github.com/servo/rust-url/) |
| `idna_adapter` | `1.2.2` | `Apache-2.0 OR MIT` | [源码](https://github.com/hsivonen/idna_adapter) |
| `image` | `0.25.10` | `MIT OR Apache-2.0` | [源码](https://github.com/image-rs/image) |
| `image-webp` | `0.2.4` | `MIT OR Apache-2.0` | [源码](https://github.com/image-rs/image-webp) |
| `imagesize` | `0.14.0` | `MIT` | [源码](https://github.com/Roughsketch/imagesize) |
| `imgref` | `1.12.2` | `CC0-1.0 OR Apache-2.0` | [源码](https://github.com/kornelski/imgref) |
| `indexmap` | `2.14.0` | `Apache-2.0 OR MIT` | [源码](https://github.com/indexmap-rs/indexmap) |
| `integer-sqrt` | `0.1.5` | `Apache-2.0/MIT` | [源码](https://github.com/derekdreery/integer-sqrt-rs) |
| `is_terminal_polyfill` | `1.70.2` | `MIT OR Apache-2.0` | [源码](https://github.com/polyfill-rs/is_terminal_polyfill) |
| `itertools` | `0.14.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-itertools/itertools) |
| `itoa` | `1.0.18` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/itoa) |
| `jiff` | `0.2.35` | `Unlicense OR MIT` | [源码](https://github.com/BurntSushi/jiff) |
| `jiff-core` | `0.1.0` | `Unlicense OR MIT` | [源码](https://github.com/BurntSushi/jiff) |
| `keyboard-types` | `0.7.0` | `MIT OR Apache-2.0` | [源码](https://github.com/pyfisch/keyboard-types) |
| `kurbo` | `0.13.1` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/kurbo) |
| `lazy_static` | `1.5.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang-nursery/lazy-static.rs) |
| `lebe` | `0.5.3` | `BSD-3-Clause` | [源码](https://github.com/johannesvollmer/lebe) |
| `libc` | `0.2.189` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/libc) |
| `libm` | `0.2.16` | `MIT` | [源码](https://github.com/rust-lang/compiler-builtins) |
| `linebender_resource_handle` | `0.1.1` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/raw_resource_handle) |
| `linked_hash_set` | `0.1.6` | `Apache-2.0` | [源码](https://github.com/alexheretic/linked-hash-set) |
| `linked-hash-map` | `0.5.6` | `MIT/Apache-2.0` | [源码](https://github.com/contain-rs/linked-hash-map) |
| `litemap` | `0.8.2` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `log` | `0.4.33` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/log) |
| `loop9` | `0.1.5` | `MIT` | [源码](https://gitlab.com/kornelski/loop9.git) |
| `lyon_algorithms` | `1.0.20` | `MIT OR Apache-2.0` | [源码](https://github.com/nical/lyon) |
| `lyon_extra` | `1.1.0` | `MIT OR Apache-2.0` | [源码](https://github.com/nical/lyon) |
| `lyon_geom` | `1.0.19` | `MIT OR Apache-2.0` | [源码](https://github.com/nical/lyon) |
| `lyon_path` | `1.0.19` | `MIT OR Apache-2.0` | [源码](https://github.com/nical/lyon) |
| `matrixmultiply` | `0.3.11` | `MIT/Apache-2.0` | [源码](https://github.com/bluss/matrixmultiply/) |
| `maybe-rayon` | `0.1.1` | `MIT` | [源码](https://github.com/shssoichiro/maybe-rayon) |
| `memchr` | `2.8.3` | `Unlicense OR MIT` | [源码](https://github.com/BurntSushi/memchr) |
| `memmap2` | `0.9.11` | `MIT OR Apache-2.0` | [源码](https://github.com/RazrFalcon/memmap2-rs) |
| `memoffset` | `0.9.1` | `MIT` | [源码](https://github.com/Gilnaa/memoffset) |
| `miniz_oxide` | `0.8.9` | `MIT OR Zlib OR Apache-2.0` | [源码](https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide) |
| `moxcms` | `0.8.1` | `BSD-3-Clause OR Apache-2.0` | [源码](https://github.com/awxkee/moxcms.git) |
| `muda` | `0.19.3` | `Apache-2.0 OR MIT` | [源码](https://github.com/tauri-apps/muda) |
| `natord` | `1.0.9` | `MIT` | [源码](https://github.com/lifthrasiir/rust-natord) |
| `ndarray` | `0.16.1` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-ndarray/ndarray) |
| `new_debug_unreachable` | `1.0.6` | `MIT` | [源码](https://github.com/mbrubeck/rust-debug-unreachable) |
| `no_std_io2` | `0.9.4` | `Apache-2.0 OR MIT` | [源码](https://github.com/wcampbell0x2a/no-std-io2) |
| `nom` | `8.0.0` | `MIT` | [源码](https://github.com/rust-bakery/nom) |
| `noop_proc_macro` | `0.3.0` | `MIT` | [源码](https://github.com/lu-zero/noop_proc_macro) |
| `num_enum` | `0.7.6` | `BSD-3-Clause OR MIT OR Apache-2.0` | [源码](https://github.com/illicitonion/num_enum) |
| `num_enum_derive` | `0.7.6` | `BSD-3-Clause OR MIT OR Apache-2.0` | [源码](https://github.com/illicitonion/num_enum) |
| `num-bigint` | `0.4.8` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-num/num-bigint) |
| `num-complex` | `0.4.6` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-num/num-complex) |
| `num-derive` | `0.4.2` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-num/num-derive) |
| `num-integer` | `0.1.46` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-num/num-integer) |
| `num-rational` | `0.4.2` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-num/num-rational) |
| `num-traits` | `0.2.19` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-num/num-traits) |
| `once_cell` | `1.21.4` | `MIT OR Apache-2.0` | [源码](https://github.com/matklad/once_cell) |
| `once_cell_polyfill` | `1.70.2` | `MIT OR Apache-2.0` | [源码](https://github.com/polyfill-rs/once_cell_polyfill) |
| `ort` | `2.0.0-rc.10` | `MIT OR Apache-2.0` | [源码](https://github.com/pykeio/ort) |
| `ort-sys` | `2.0.0-rc.10` | `MIT OR Apache-2.0` | [源码](https://github.com/pykeio/ort) |
| `parlance` | `0.1.0` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/parley) |
| `parley` | `0.10.0` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/parley) |
| `parley_data` | `0.10.0` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/parley) |
| `paste` | `1.0.15` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/paste) |
| `pastey` | `0.1.1` | `MIT OR Apache-2.0` | [源码](https://github.com/as1100k/pastey) |
| `percent-encoding` | `2.3.2` | `MIT OR Apache-2.0` | [源码](https://github.com/servo/rust-url/) |
| `pico-args` | `0.5.0` | `MIT` | [源码](https://github.com/RazrFalcon/pico-args) |
| `pin-project` | `1.1.13` | `Apache-2.0 OR MIT` | [源码](https://github.com/taiki-e/pin-project) |
| `pin-project-internal` | `1.1.13` | `Apache-2.0 OR MIT` | [源码](https://github.com/taiki-e/pin-project) |
| `pin-project-lite` | `0.2.17` | `Apache-2.0 OR MIT` | [源码](https://github.com/taiki-e/pin-project-lite) |
| `pin-utils` | `0.1.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang-nursery/pin-utils) |
| `pin-weak` | `1.1.0` | `MIT` | [源码](https://github.com/sixtyfpsui/pin-weak) |
| `png` | `0.18.1` | `MIT OR Apache-2.0` | [源码](https://github.com/image-rs/image-png) |
| `polycool` | `0.4.0` | `MIT OR Apache-2.0` | [源码](https://github.com/linebender/kurbo) |
| `portable-atomic` | `1.14.0` | `Apache-2.0 OR MIT` | [源码](https://github.com/taiki-e/portable-atomic) |
| `potential_utf` | `0.1.5` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `ppv-lite86` | `0.2.21` | `MIT OR Apache-2.0` | [源码](https://github.com/cryptocorrosion/cryptocorrosion) |
| `proc-macro-crate` | `3.5.0` | `MIT OR Apache-2.0` | [源码](https://github.com/bkchr/proc-macro-crate) |
| `proc-macro2` | `1.0.107` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/proc-macro2) |
| `profiling` | `1.0.18` | `MIT OR Apache-2.0` | [源码](https://github.com/aclysma/profiling) |
| `profiling-procmacros` | `1.0.18` | `MIT OR Apache-2.0` | [源码](https://github.com/aclysma/profiling) |
| `pulldown-cmark` | `0.13.4` | `MIT` | [源码](https://github.com/raphlinus/pulldown-cmark) |
| `pulldown-cmark-escape` | `0.11.0` | `MIT` | [源码](https://github.com/raphlinus/pulldown-cmark) |
| `pulp` | `0.22.3` | `MIT` | [源码](https://github.com/sarah-quinones/pulp/) |
| `pulp-wasm-simd-flag` | `0.1.1` | `MIT` | [源码](https://github.com/sarah-quinones/pulp/) |
| `pxfm` | `0.1.30` | `BSD-3-Clause OR Apache-2.0` | [源码](https://github.com/awxkee/pxfm) |
| `qoi` | `0.4.1` | `MIT/Apache-2.0` | [源码](https://github.com/aldanor/qoi-rust) |
| `quick-error` | `2.0.1` | `MIT/Apache-2.0` | [源码](http://github.com/tailhook/quick-error) |
| `quote` | `1.0.47` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/quote) |
| `rand` | `0.8.7` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-random/rand) |
| `rand_chacha` | `0.3.1` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-random/rand) |
| `rand_core` | `0.6.4` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-random/rand) |
| `rav1e` | `0.8.1` | `BSD-2-Clause` | [源码](https://github.com/xiph/rav1e/) |
| `ravif` | `0.13.0` | `BSD-3-Clause` | [源码](https://github.com/kornelski/cavif-rs) |
| `raw-cpuid` | `11.6.0` | `MIT` | [源码](https://github.com/gz/rust-cpuid) |
| `raw-window-handle` | `0.6.2` | `MIT OR Apache-2.0 OR Zlib` | [源码](https://github.com/rust-windowing/raw-window-handle) |
| `rawpointer` | `0.2.1` | `MIT/Apache-2.0` | [源码](https://github.com/bluss/rawpointer/) |
| `rayon` | `1.12.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rayon-rs/rayon) |
| `rayon-core` | `1.13.0` | `MIT OR Apache-2.0` | [源码](https://github.com/rayon-rs/rayon) |
| `read-fonts` | `0.39.2` | `MIT OR Apache-2.0` | [源码](https://github.com/googlefonts/fontations) |
| `read-fonts` | `0.41.0` | `MIT OR Apache-2.0` | [源码](https://github.com/googlefonts/fontations) |
| `reborrow` | `0.5.5` | `MIT` | [源码](https://github.com/sarah-ek/reborrow/) |
| `regex` | `1.13.1` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/regex) |
| `regex-automata` | `0.4.16` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/regex) |
| `regex-syntax` | `0.8.11` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/regex) |
| `resvg` | `0.47.0` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/resvg) |
| `rgb` | `0.8.53` | `MIT` | [源码](https://github.com/kornelski/rust-rgb) |
| `rowan` | `0.16.1` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-analyzer/rowan) |
| `roxmltree` | `0.21.1` | `MIT OR Apache-2.0` | [源码](https://github.com/RazrFalcon/roxmltree) |
| `rspolib` | `0.1.2` | `MIT` | [源码](https://github.com/mondeja/rspolib) |
| `rustc-hash` | `1.1.0` | `Apache-2.0/MIT` | [源码](https://github.com/rust-lang-nursery/rustc-hash) |
| `rustversion` | `1.0.23` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/rustversion) |
| `rustybuzz` | `0.20.1` | `MIT` | [源码](https://github.com/harfbuzz/rustybuzz) |
| `scoped-tls-hkt` | `0.1.5` | `MIT/Apache-2.0` | [源码](https://github.com/Diggsey/scoped-tls-hkt) |
| `scopeguard` | `1.2.0` | `MIT OR Apache-2.0` | [源码](https://github.com/bluss/scopeguard) |
| `serde` | `1.0.229` | `MIT OR Apache-2.0` | [源码](https://github.com/serde-rs/serde) |
| `serde_core` | `1.0.229` | `MIT OR Apache-2.0` | [源码](https://github.com/serde-rs/serde) |
| `serde_derive` | `1.0.229` | `MIT OR Apache-2.0` | [源码](https://github.com/serde-rs/serde) |
| `serde_json` | `1.0.151` | `MIT OR Apache-2.0` | [源码](https://github.com/serde-rs/json) |
| `sha1` | `0.10.7` | `MIT OR Apache-2.0` | [源码](https://github.com/RustCrypto/hashes) |
| `simd_helpers` | `0.1.0` | `MIT` | [源码](https://github.com/lu-zero/simd_helpers) |
| `simd-adler32` | `0.3.10` | `MIT` | [源码](https://github.com/mcountryman/simd-adler32) |
| `simplecss` | `0.2.2` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/simplecss) |
| `siphasher` | `1.0.3` | `MIT/Apache-2.0` | [源码](https://github.com/jedisct1/rust-siphash) |
| `skrifa` | `0.42.1` | `MIT OR Apache-2.0` | [源码](https://github.com/googlefonts/fontations) |
| `skrifa` | `0.44.0` | `MIT OR Apache-2.0` | [源码](https://github.com/googlefonts/fontations) |
| `slab` | `0.4.12` | `MIT` | [源码](https://github.com/tokio-rs/slab) |
| `slint` | `1.17.1` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | [源码](https://github.com/slint-ui/slint) |
| `slint-macros` | `1.17.1` | `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | [源码](https://github.com/slint-ui/slint) |
| `slotmap` | `1.1.1` | `Zlib` | [源码](https://github.com/orlp/slotmap) |
| `smallvec` | `1.15.2` | `MIT OR Apache-2.0` | [源码](https://github.com/servo/rust-smallvec) |
| `smallvec` | `2.0.0-alpha.10` | `MIT OR Apache-2.0` | [源码](https://github.com/servo/rust-smallvec) |
| `smol_str` | `0.2.2` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-analyzer/smol_str) |
| `smol_str` | `0.3.6` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-lang/rust-analyzer/tree/master/lib/smol_str) |
| `snafu` | `0.8.9` | `MIT OR Apache-2.0` | [源码](https://github.com/shepmaster/snafu) |
| `snafu-derive` | `0.8.9` | `MIT OR Apache-2.0` | [源码](https://github.com/shepmaster/snafu) |
| `softbuffer` | `0.4.8` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-windowing/softbuffer) |
| `spin_on` | `0.1.1` | `Apache-2.0 OR MIT` | — |
| `stable_deref_trait` | `1.2.1` | `MIT OR Apache-2.0` | [源码](https://github.com/storyyeller/stable_deref_trait) |
| `strict-num` | `0.1.1` | `MIT` | [源码](https://github.com/RazrFalcon/strict-num) |
| `strum` | `0.28.0` | `MIT` | [源码](https://github.com/Peternator7/strum) |
| `strum_macros` | `0.28.0` | `MIT` | [源码](https://github.com/Peternator7/strum) |
| `svgtypes` | `0.16.1` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/svgtypes) |
| `swash` | `0.2.10` | `Apache-2.0 OR MIT` | [源码](https://github.com/dfrg/swash) |
| `syn` | `2.0.119` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/syn) |
| `syn` | `3.0.3` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/syn) |
| `synstructure` | `0.13.2` | `MIT` | [源码](https://github.com/mystor/synstructure) |
| `sys-locale` | `0.3.2` | `MIT OR Apache-2.0` | [源码](https://github.com/1Password/sys-locale) |
| `taffy` | `0.10.1` | `MIT` | [源码](https://github.com/DioxusLabs/taffy) |
| `text-size` | `1.1.1` | `MIT OR Apache-2.0` | [源码](https://github.com/rust-analyzer/text-size) |
| `thiserror` | `1.0.69` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/thiserror) |
| `thiserror` | `2.0.19` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/thiserror) |
| `thiserror-impl` | `1.0.69` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/thiserror) |
| `thiserror-impl` | `2.0.19` | `MIT OR Apache-2.0` | [源码](https://github.com/dtolnay/thiserror) |
| `tiff` | `0.11.3` | `MIT` | [源码](https://github.com/image-rs/image-tiff) |
| `tiny-skia` | `0.12.0` | `BSD-3-Clause` | [源码](https://github.com/linebender/tiny-skia) |
| `tiny-skia-path` | `0.12.0` | `BSD-3-Clause` | [源码](https://github.com/linebender/tiny-skia/tree/master/path) |
| `tinystr` | `0.8.3` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `tinyvec` | `1.12.0` | `Zlib OR Apache-2.0 OR MIT` | [源码](https://github.com/Lokathor/tinyvec) |
| `tinyvec_macros` | `0.1.1` | `MIT OR Apache-2.0 OR Zlib` | [源码](https://github.com/Soveu/tinyvec_macros) |
| `toml_datetime` | `1.1.1+spec-1.1.0` | `MIT OR Apache-2.0` | [源码](https://github.com/toml-rs/toml) |
| `toml_edit` | `0.25.13+spec-1.1.0` | `MIT OR Apache-2.0` | [源码](https://github.com/toml-rs/toml) |
| `toml_parser` | `1.1.2+spec-1.1.0` | `MIT OR Apache-2.0` | [源码](https://github.com/toml-rs/toml) |
| `toml_writer` | `1.1.2+spec-1.1.0` | `MIT OR Apache-2.0` | [源码](https://github.com/toml-rs/toml) |
| `tracing` | `0.1.44` | `MIT` | [源码](https://github.com/tokio-rs/tracing) |
| `tracing-core` | `0.1.36` | `MIT` | [源码](https://github.com/tokio-rs/tracing) |
| `ttf-parser` | `0.25.1` | `MIT OR Apache-2.0` | [源码](https://github.com/harfbuzz/ttf-parser) |
| `tungstenite` | `0.24.0` | `MIT OR Apache-2.0` | [源码](https://github.com/snapview/tungstenite-rs) |
| `typed-index-collections` | `3.5.0` | `MIT OR Apache-2.0` | [源码](https://github.com/zheland/typed-index-collections) |
| `typenum` | `1.20.1` | `MIT OR Apache-2.0` | [源码](https://github.com/paholg/typenum) |
| `unicase` | `2.9.0` | `MIT OR Apache-2.0` | [源码](https://github.com/seanmonstar/unicase) |
| `unicode-bidi` | `0.3.18` | `MIT OR Apache-2.0` | [源码](https://github.com/servo/unicode-bidi) |
| `unicode-bidi-mirroring` | `0.4.0` | `MIT/Apache-2.0` | [源码](https://github.com/RazrFalcon/unicode-bidi-mirroring) |
| `unicode-ccc` | `0.4.0` | `MIT/Apache-2.0` | [源码](https://github.com/RazrFalcon/unicode-ccc) |
| `unicode-ident` | `1.0.24` | `(MIT OR Apache-2.0) AND Unicode-3.0` | [源码](https://github.com/dtolnay/unicode-ident) |
| `unicode-linebreak` | `0.1.5` | `Apache-2.0` | [源码](https://github.com/axelf4/unicode-linebreak) |
| `unicode-properties` | `0.1.4` | `MIT/Apache-2.0` | [源码](https://github.com/unicode-rs/unicode-properties) |
| `unicode-script` | `0.5.8` | `MIT OR Apache-2.0` | [源码](https://github.com/unicode-rs/unicode-script) |
| `unicode-segmentation` | `1.13.3` | `MIT OR Apache-2.0` | [源码](https://github.com/unicode-rs/unicode-segmentation) |
| `unicode-vo` | `0.1.0` | `MIT/Apache-2.0` | [源码](https://github.com/RazrFalcon/unicode-vo) |
| `unicode-width` | `0.2.2` | `MIT OR Apache-2.0` | [源码](https://github.com/unicode-rs/unicode-width) |
| `unicode-xid` | `0.2.6` | `MIT OR Apache-2.0` | [源码](https://github.com/unicode-rs/unicode-xid) |
| `url` | `2.5.8` | `MIT OR Apache-2.0` | [源码](https://github.com/servo/rust-url) |
| `usvg` | `0.47.0` | `Apache-2.0 OR MIT` | [源码](https://github.com/linebender/resvg) |
| `utf-8` | `0.7.6` | `MIT OR Apache-2.0` | [源码](https://github.com/SimonSapin/rust-utf8) |
| `utf8_iter` | `1.0.4` | `Apache-2.0 OR MIT` | [源码](https://github.com/hsivonen/utf8_iter) |
| `utf8parse` | `0.2.2` | `Apache-2.0 OR MIT` | [源码](https://github.com/alacritty/vte) |
| `v_frame` | `0.3.9` | `BSD-2-Clause` | [源码](https://github.com/rust-av/v_frame) |
| `vtable` | `0.4.0` | `MIT OR Apache-2.0` | [源码](https://github.com/slint-ui/slint) |
| `vtable-macro` | `0.4.0` | `MIT OR Apache-2.0` | [源码](https://github.com/slint-ui/slint) |
| `webbrowser` | `1.2.1` | `MIT OR Apache-2.0` | [源码](https://github.com/amodm/webbrowser-rs) |
| `weezl` | `0.1.12` | `MIT OR Apache-2.0` | [源码](https://github.com/image-rs/weezl) |
| `windows` | `0.58.0` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows` | `0.62.2` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows_x86_64_msvc` | `0.52.6` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-collections` | `0.3.2` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-core` | `0.58.0` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-core` | `0.62.2` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-future` | `0.3.2` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-implement` | `0.58.0` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-implement` | `0.60.2` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-interface` | `0.58.0` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-interface` | `0.59.3` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-link` | `0.2.1` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-numerics` | `0.3.1` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-result` | `0.2.0` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-result` | `0.4.1` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-strings` | `0.1.0` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-strings` | `0.5.1` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-sys` | `0.52.0` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-sys` | `0.61.2` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-targets` | `0.52.6` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `windows-threading` | `0.2.1` | `MIT OR Apache-2.0` | [源码](https://github.com/microsoft/windows-rs) |
| `winit` | `0.30.13` | `Apache-2.0` | [源码](https://github.com/rust-windowing/winit) |
| `winnow` | `1.0.4` | `MIT` | [源码](https://github.com/winnow-rs/winnow) |
| `writeable` | `0.6.3` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `xmlwriter` | `0.1.0` | `MIT` | [源码](https://github.com/RazrFalcon/xmlwriter) |
| `y4m` | `0.8.0` | `MIT` | [源码](https://github.com/image-rs/y4m.git) |
| `yazi` | `0.2.1` | `Apache-2.0 OR MIT` | [源码](https://github.com/dfrg/yazi) |
| `yoke` | `0.8.3` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `yoke-derive` | `0.8.2` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `zeno` | `0.3.3` | `Apache-2.0 OR MIT` | [源码](https://github.com/dfrg/zeno) |
| `zerocopy` | `0.8.55` | `BSD-2-Clause OR Apache-2.0 OR MIT` | [源码](https://github.com/google/zerocopy) |
| `zerocopy-derive` | `0.8.55` | `BSD-2-Clause OR Apache-2.0 OR MIT` | [源码](https://github.com/google/zerocopy) |
| `zerofrom` | `0.1.8` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `zerofrom-derive` | `0.1.7` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `zerotrie` | `0.2.4` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `zerovec` | `0.11.6` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `zerovec-derive` | `0.11.3` | `Unicode-3.0` | [源码](https://github.com/unicode-org/icu4x) |
| `zmij` | `1.0.23` | `MIT` | [源码](https://github.com/dtolnay/zmij) |
| `zune-core` | `0.5.1` | `MIT OR Apache-2.0 OR Zlib` | [源码](https://github.com/etemesi254/zune-image) |
| `zune-inflate` | `0.2.54` | `MIT OR Apache-2.0 OR Zlib` | — |
| `zune-jpeg` | `0.5.15` | `MIT OR Apache-2.0 OR Zlib` | [源码](https://github.com/etemesi254/zune-image/tree/dev/crates/zune-jpeg) |
