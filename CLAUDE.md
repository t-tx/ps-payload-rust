# ps-payload-rust

A Rust payload for exploited PS5s, built in layers on top of the
[ps5-payload-sdk](https://github.com/ps5-payload-dev/sdk). The final artifact is
an `.elf` loaded by an ELF loader (elfldr, websrv, BD-J, etc.).

## Layered architecture

The project is intentionally split so each layer only depends on the one below
it. Keep unsafe code at the bottom; keep application logic at the top.

| Layer | Crate            | Responsibility                                              | Status      |
|-------|------------------|-------------------------------------------------------------|-------------|
| **L1**| `crates/ps5-sys` | Raw `unsafe` FFI bindings generated from the SDK C headers. | done        |
| **L2**| `crates/ps5`     | Safe, idiomatic wrappers over L1 (RAII, `Result`, slices).  | in progress |
| **L3**| `crates/core`    | Business-core use cases; target-agnostic, no FFI.           | in progress |
| **L4**| `crates/app`     | Application feature; the crate that links into the `.elf`.  | todo        |

Rules:
- A layer may call **only the layer directly beneath it**. App → core → ps5 → ps5-sys.
- `unsafe` lives in **L1** (declarations) and **L2** (where it is encapsulated and
  made safe). L3/L4 should contain no `unsafe`.
- L1 is regenerated, never hand-edited. L2 is where naming/ergonomics happen.

## Repository layout

```
.
├── Cargo.toml            # workspace
├── CLAUDE.md             # this file
├── Makefile              # `make generate`, build, fmt, … (see `make help`)
├── docs/
│   └── SDK_API_INVENTORY.md   # full file-grounded map of what the SDK provides
├── sdk/                  # git submodule: ps5-payload-dev/sdk
└── crates/
    ├── ps5-sys/          # L1 — raw FFI
    │   ├── build.rs      # one bindgen run per domain → $OUT_DIR/*_bindings.rs
    │   ├── headers/      # ps5.h, fs.h, net.h, thread.h (the surfaces to bind)
    │   └── src/
    │       ├── lib.rs    # #![no_std]; include!s the generated bindings as modules
    │       └── sce.rs    # hand-written SceHttp2/SceSsl/SceNet FFI (symbol-only)
    ├── ps5/              # L2 — safe wrappers
    │   └── src/
    │       ├── lib.rs    # #![no_std] + alloc; re-exports error; feature-gated modules
    │       ├── error.rs  # Error / Result / Errno / SceError
    │       ├── util.rs   # cstr(), cvt_i32(), cvt_ssize(), cvt_ptr()
    │       └── fs.rs net.rs http.rs thread.rs   # the wrapper modules
    └── core/             # L3 — business core (package `ps5-core`; no FFI/unsafe)
        └── src/
            ├── lib.rs    # #![no_std] + alloc
            └── http.rs   # minimal HTTP/1.1 server (Server/Request/Response)
```

## Toolchain & prerequisites

The SDK submodule must be present:

```sh
git submodule update --init sdk
```

**For L1 (generating bindings):** only `libclang` is required (bindgen uses it).
On macOS the Xcode/CommandLineTools `libclang` is found automatically; if not,
set `LIBCLANG_PATH` (e.g. `/Library/Developer/CommandLineTools/usr/lib`). On
Debian/Ubuntu: `apt install libclang-dev`.

**For L4 (compiling/linking the actual payload):** the full SDK toolchain is
needed (clang-18 + lld-18 and an *installed* SDK that provides `crt1.o`/`libc.a`):

```sh
# macOS
brew install llvm@18
export LLVM_CONFIG=/opt/homebrew/opt/llvm@18/bin/llvm-config
# Debian/Ubuntu:  apt install clang-18 lld-18

# build + install the SDK to a prefix, then point the env at it
make -C sdk DESTDIR=/opt/ps5-payload-sdk install
export PS5_PAYLOAD_SDK=/opt/ps5-payload-sdk
```

## L1 — `ps5-sys` (FFI bindings)

`crates/ps5-sys/build.rs` runs **one bindgen pass per domain** (each over a
header in `headers/`) into `$OUT_DIR/<domain>_bindings.rs`; `src/lib.rs`
`include!`s each as a module. Bindings are **never committed** — they live under
`target/`. The exhaustive map of what the SDK offers is in
[docs/SDK_API_INVENTORY.md](docs/SDK_API_INVENTORY.md).

| Surface | Module | Feature | Source | What |
|---|---|---|---|---|
| `ps5/*` + errno | crate root | always | `headers/ps5.h` | `payload_*`, `kernel_*`, `klog_*`, `mdbg_*`, `nid_*`, `KERNEL_*`, `__error`, `strerror`, `E*` |
| file/fs | `fs` | `fs` | `headers/fs.h` | `open`/`read`/`write`/`stat`/`opendir`/`mmap`/`fopen`, `O_*`/`S_*` |
| sockets | `net` | `net` | `headers/net.h` | `socket`/`connect`/`getaddrinfo`/`poll`, `sockaddr_in`, `AF_*` |
| threads | `thread` | `thread` | `headers/thread.h` | `pthread_*`, `sem_*`, `sched_*`, C11 `thrd_*` |
| HTTP/TLS | `sce` | `http` | **hand-written** `src/sce.rs` | `sceHttp2*`/`sceSsl*`/`sceNet*` (symbol-only, no SDK headers) |

Features default to all four on; disable to shrink the surface
(`--no-default-features --features fs,net`). The `http` feature also emits
`-lSceHttp2 -lSceSsl` for the final link.

### SDK discovery (build.rs)
1. `$PS5_PAYLOAD_SDK` if set → installed layout (`$SDK/target/include`).
2. Otherwise the in-tree `sdk/` submodule → source layout (`sdk/include`).

So bindings can be generated straight from the submodule with no SDK install.

### Three things to know (already handled, don't "fix" them)
- **Parse target is `x86_64-unknown-freebsd`, not `x86_64-sie-ps5`.** On the Sony
  triple, clang's `-fvisibility-from-dllstorageclass` handling makes libclang drop
  every `extern` function from codegen, so bindgen emits the structs/statics but
  *zero functions*. PS5 (Orbis) is x86_64/LP64/SysV-ELF and the SDK's libc headers
  are FreeBSD's, so the freebsd triple yields an ABI-identical FFI surface with
  functions intact. See the `TARGET` doc-comment in `build.rs`.
- **Include isolation.** bindgen parses with `-nostdinc` + the clang resource dir
  + the SDK's FreeBSD headers, so the host's system headers (macOS/Linux) never
  leak in and substitute wrong struct layouts.
- **Linkage.** ps5/* symbols come from `crt1.o`; POSIX/BSD + C-library symbols
  from `kernel_web`/`SceLibcInternal`/the SDK `libc.a` — all default-linked by
  `prospero-clang`. Only `http` adds link flags (`-lSceHttp2 -lSceSsl`).

### Generate / inspect
```sh
make generate          # force all bindgen runs (debug), then report each module's surface
make generate RELEASE=1 # or: make generate-release
make show-bindings     # per-module fn/const/struct counts
make print-bindings    # dump all generated *_bindings.rs
```
The Makefile auto-discovers `libclang` (set `LIBCLANG_PATH` to override) and
touches `build.rs` so every run re-generates. `RELEASE=1` is accepted by the
build-ish targets (`generate`, `build`, `check`, `clippy`). Equivalent by hand:
`cargo build -p ps5-sys [--release]`; generated files live at
`target/<profile>/build/ps5-sys-*/out/<domain>_bindings.rs`. `make help` lists all targets.

## L2 — `ps5` (safe wrappers)

`#![no_std]` + `alloc`. Turns the raw L1 FFI into std-like, memory-safe APIs;
`unsafe` is confined here and below. All calls return [`Result`] with the shared
`Error` (`Os(Errno)` for POSIX failures, `Sce` for HTTP/TLS, `InvalidInput`).
Internal helpers in `util.rs` (`cstr`, `cvt_i32`, `cvt_ssize`, `cvt_ptr`) keep the
syscall→`Result` mapping consistent.

Modules (feature-gated, forwarding to the matching `ps5-sys` feature):
- `fs`     — `File`, `OpenOptions`, `read`/`write`/`metadata`/`read_dir`.
- `net`    — `TcpStream`, `TcpListener`, `UdpSocket`, `lookup_host` (uses `core::net`). IPv4 only so far.
- `http`   — `Client` + `Response` over Sony `SceHttp2` (RAII Net→Ssl→Http2 lifecycle).
- `thread` — `spawn`/`JoinHandle`, `Builder` (`stack_size`), `Mutex`, `Condvar` (over pthreads).

## L3 — `ps5-core` (business core)

`#![no_std]` + `alloc`, **no FFI / no `unsafe`** — plain logic over the L2 APIs.
The package is `ps5-core` (directory `crates/core`; not named `core`, which would
collide with the language crate).

- `http` — a minimal **HTTP/1.1 server** (`Server`, `Request`, `Response`).
  Thread-per-connection over `ps5::net::TcpListener`, each handler thread spawned
  with a small capped stack via `ps5::thread::Builder` (default 128 KiB).
  `Connection: close`, one request per connection, `Content-Length` bodies only.
  There is **no Sony HTTP-server lib**, so the protocol is implemented here on
  sockets (see `docs/SDK_API_INVENTORY.md` — `SceHttp2`/`SceHttp` are clients).
  Concurrency is unbounded (one thread per live connection); add a cap if needed.

Bump the `bindgen` version in `crates/ps5-sys/Cargo.toml` to update the generator.

## L4 — building the `.elf` (intended approach, to implement)

Rust has no `std` for `x86_64-sie-ps5`, so the payload is `#![no_std]` and built
with a **custom target spec** + `build-std` on nightly, linked through the SDK's
clang driver:

- Add `targets/x86_64-sie-ps5.json` (base it on the freebsd spec, set
  `linker-flavor` to a clang-style driver and `linker` to
  `$PS5_PAYLOAD_SDK/bin/prospero-clang`). `prospero-clang` supplies `crt1.o`
  (which provides `_start`), `libc`, and the kernel/SCE stubs.
- Provide a `#[panic_handler]` and the payload entry expected by `crt1.o`.
- Build with nightly:
  ```sh
  rustup toolchain install nightly
  rustup component add rust-src --toolchain nightly
  cargo +nightly build -Z build-std=core,alloc \
        --target targets/x86_64-sie-ps5.json --release -p app
  ```
- Deploy to a console running an ELF loader:
  ```sh
  $PS5_PAYLOAD_SDK/bin/prospero-deploy -h $PS5_HOST -p 9021 app.elf
  ```

Reference C samples live in `sdk/samples/` (start with `hello_world`).

## Conventions

- Don't hand-edit generated bindings; change the `headers/*.h` surface or `build.rs` and rebuild.
- New SDK surface → extend the allowlists in `build.rs`, then add a safe wrapper in L2.
- Keep `unsafe` out of L3/L4.
- License: derived from GPLv3+ SDK headers — crates are `GPL-3.0-or-later`.
