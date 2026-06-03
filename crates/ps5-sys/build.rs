//! L1 generator: parse the ps5-payload-sdk headers with bindgen and emit raw
//! Rust FFI declarations into `$OUT_DIR/*_bindings.rs`.
//!
//! One run per domain so the crate exposes feature-gated modules:
//!   - `headers/ps5.h`    → `ps5_bindings.rs`    (always; ps5/* + errno + strerror)
//!   - `headers/fs.h`     → `fs_bindings.rs`     (feature `fs`)
//!   - `headers/net.h`    → `net_bindings.rs`    (feature `net`)
//!   - `headers/thread.h` → `thread_bindings.rs` (feature `thread`)
//!
//! HTTP/TLS (feature `http`) is symbol-only with no SDK headers — it is
//! hand-written in `src/sce.rs`; here we only emit its link directives.
//!
//! Include isolation: we parse with `x86_64-unknown-freebsd` (see TARGET) and
//! `-nostdinc` + the clang resource dir, so ONLY the SDK's FreeBSD headers and
//! the compiler builtins are seen — never the host's system headers (which on
//! macOS would silently substitute wrong struct layouts).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Clang target used to *parse* the headers.
///
/// We deliberately do NOT use the payload's own triple (`x86_64-sie-ps5`) here.
/// On the Sony triple, clang's `-fvisibility-from-dllstorageclass` handling makes
/// libclang omit every `extern` function declaration from its codegen cursors, so
/// bindgen emits structs/typedefs/statics but ZERO functions.
///
/// `x86_64-unknown-freebsd` is ABI-identical for our purposes — PS5 (Orbis) is an
/// x86_64, LP64, System-V ELF platform and the SDK's libc headers literally are
/// FreeBSD's — so the generated FFI layout matches what the payload links against,
/// while function declarations come through.
const TARGET: &str = "x86_64-unknown-freebsd";

enum Sdk {
    /// Installed SDK root; headers at `<root>/target/include`.
    Installed(PathBuf),
    /// SDK source tree (the submodule); headers at `<root>/include`.
    Source(PathBuf),
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PS5_PAYLOAD_SDK");
    for h in ["ps5", "fs", "net", "thread"] {
        println!("cargo:rerun-if-changed=headers/{h}.h");
    }

    let sdk = locate_sdk();
    let base = clang_args(&sdk);
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));

    // Core (always): the ps5/* API + errno codes + __error + strerror, emitted
    // at the crate root.
    generate(&base, "headers/ps5.h", &out.join("ps5_bindings.rs"), |b| {
        b.allowlist_file(r".*/ps5/.*\.h")
            .allowlist_function("(kernel|klog|mdbg|payload|nid)_.*")
            .allowlist_var("KERNEL_.*")
            .allowlist_type("payload_args.*")
            .allowlist_function("__error")
            .allowlist_function("strerror")
            .allowlist_var("E[A-Z0-9]+") // errno numeric codes (EINVAL, EAGAIN, …)
    });

    if feature("fs") {
        generate(&base, "headers/fs.h", &out.join("fs_bindings.rs"), |b| {
            b.allowlist_file(r".*/(fcntl|unistd|dirent|stdio)\.h")
                .allowlist_file(r".*/sys/(stat|mman|uio|types)\.h")
                .allowlist_var("(O|S|F|SEEK|PROT|MAP|MS|AT)_.*")
        });
    }

    if feature("net") {
        generate(&base, "headers/net.h", &out.join("net_bindings.rs"), |b| {
            b.allowlist_file(r".*/sys/(socket|select|ioctl|ioccom)\.h")
                .allowlist_file(r".*/netdb\.h")
                .allowlist_file(r".*/netinet/(in|tcp)\.h")
                .allowlist_file(r".*/netinet6/.*\.h")
                .allowlist_file(r".*/arpa/inet\.h")
                .allowlist_file(r".*/net/if\.h")
                .allowlist_file(r".*/(poll\.h|sys/poll\.h)")
                .allowlist_file(r".*/fcntl\.h")
                .allowlist_function("close") // sockets are fds (from unistd.h)
                .allowlist_var(
                    "(AF|PF|SOCK|SOL|SO|IPPROTO|IP|IPV6|TCP|MSG|SHUT|INADDR|IN6ADDR|POLL|FIONBIO|FD|F|O|AI|NI)_.*",
                )
        });
    }

    if feature("thread") {
        generate(
            &base,
            "headers/thread.h",
            &out.join("thread_bindings.rs"),
            |b| {
                b.allowlist_file(r".*/(pthread|pthread_np|semaphore|threads)\.h")
                    .allowlist_file(r".*/(sched\.h|sys/sched\.h)")
                    .allowlist_var("(PTHREAD|SCHED|SEM|ONCE|TSS|MTX|THRD)_.*")
            },
        );
    }

    if feature("http") {
        // SceHttp2 + SceSsl are opt-in libs; SceNet is default-linked but stating
        // it is harmless. These propagate to the final payload link (prospero-clang).
        // Inert for host rlib builds (libs are not linked).
        println!("cargo:rustc-link-lib=SceHttp2");
        println!("cargo:rustc-link-lib=SceSsl");
        println!("cargo:rustc-link-lib=SceNet");
    }
}

fn feature(name: &str) -> bool {
    env::var(format!("CARGO_FEATURE_{}", name.to_uppercase())).is_ok()
}

/// Run bindgen for one header with the shared base args plus a per-domain
/// allowlist closure, writing the result to `out`.
fn generate(
    base: &[String],
    header: &str,
    out: &Path,
    configure: impl FnOnce(bindgen::Builder) -> bindgen::Builder,
) {
    let builder = bindgen::Builder::default()
        .header(header)
        .clang_args(base)
        .use_core() // no_std: `core::ffi::c_*`, `::core` paths
        .generate_comments(true)
        .layout_tests(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));

    configure(builder)
        .generate()
        .unwrap_or_else(|e| panic!("bindgen failed for {header}: {e}"))
        .write_to_file(out)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", out.display()));
}

fn locate_sdk() -> Sdk {
    if let Ok(root) = env::var("PS5_PAYLOAD_SDK") {
        let root = PathBuf::from(root);
        if root.join("target/include/ps5/payload.h").is_file() {
            return Sdk::Installed(root);
        }
        println!(
            "cargo:warning=PS5_PAYLOAD_SDK={} set but target/include/ps5/payload.h \
             not found there; falling back to the in-tree sdk/ submodule.",
            root.display()
        );
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let sdk_src = manifest
        .join("../../sdk")
        .canonicalize()
        .unwrap_or_else(|_| manifest.join("../../sdk"));
    if sdk_src.join("include/ps5/payload.h").is_file() {
        return Sdk::Source(sdk_src);
    }

    panic!(
        "Could not locate the ps5-payload-sdk.\n\
         Set PS5_PAYLOAD_SDK to an installed SDK (with target/include/ps5/payload.h),\n\
         or initialize the submodule:  git submodule update --init sdk\n\
         (looked for installed SDK via $PS5_PAYLOAD_SDK and source at {})",
        sdk_src.display()
    );
}

/// Clang arguments for parsing. Only the target and include search paths affect
/// the FFI layout; codegen-only flags from `prospero-clang` are omitted.
fn clang_args(sdk: &Sdk) -> Vec<String> {
    let mut args = vec!["-target".into(), TARGET.into(), "-ffreestanding".into()];

    // Isolate from host system headers: with -nostdinc only the clang builtin
    // resource headers + the SDK headers below are visible.
    match clang_resource_dir() {
        Some(res) => {
            args.push("-nostdinc".into());
            args.push("-isystem".into());
            args.push(format!("{res}/include"));
        }
        None => println!(
            "cargo:warning=could not determine clang resource dir; proceeding \
             without -nostdinc — host system headers may leak into the bindings."
        ),
    }

    match sdk {
        Sdk::Installed(root) => {
            args.push("-isystem".into());
            args.push(path(root.join("target/include")));
        }
        Sdk::Source(root) => {
            args.push("-isystem".into());
            args.push(path(root.join("include/freebsd")));
            args.push("-I".into());
            args.push(path(root.join("include")));
        }
    }
    args
}

/// Locate the clang builtin-headers dir (`<resource>/include`, holding
/// `stddef.h`/`stdint.h`/`stdarg.h`) so `-nostdinc` doesn't lose the builtins.
fn clang_resource_dir() -> Option<String> {
    let clang = env::var("CLANG_PATH").unwrap_or_else(|_| "clang".into());
    let output = Command::new(clang)
        .arg("-print-resource-dir")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let dir = String::from_utf8(output.stdout).ok()?.trim().to_string();
    Path::new(&dir)
        .join("include/stddef.h")
        .is_file()
        .then_some(dir)
}

fn path(p: impl AsRef<Path>) -> String {
    p.as_ref().to_string_lossy().into_owned()
}
