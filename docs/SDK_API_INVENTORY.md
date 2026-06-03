# PS5 Payload SDK — API Inventory

This document is a file-grounded inventory of every API surface provided by the vendored `sdk/` submodule (the ps5-payload-sdk) that the Rust project layers on top of. Every entry is cited to an actual file under `sdk/`. The goal is to orient the **L2** wrapping effort: only the `ps5/*` kernel API (L1) is wrapped today by `crates/ps5-sys`; **everything else listed here is not yet wrapped**. For each API the inventory records its **provider** (which library satisfies the symbol and whether it is linked by default), and — critically — whether it is **declared** (a C prototype exists, so bindgen can generate it) or **symbol-only** (a stub exports the symbol but no header exists, so you must hand-write the `extern "C"` prototype yourself).

---

## How the SDK is provided (linkage & legend)

### Provider / linkage model

The toolchain driver is `host/bin/prospero-clang`. Its default link line is:

- `LIBS_CRT` → **`crt1.o`** (always linked). This pulls in the entire `ps5/*` kernel API (`kernel_*`, `klog_*`, `mdbg_*`, `nid_*`, `payload_*`).
- `LIBS_KERN="-lkernel_web"` (`prospero-clang:28`) → low-level POSIX/BSD syscalls (`socket`, `open`, `read`, `pthread_*`, `mmap`, …).
- `LIBS_DEPS="-lSceLibcInternal -lSceNet"` (`prospero-clang:27`) → the standard C library (`malloc`, stdio, string, math) plus the Sony `sceNet*` socket layer.
- The SDK's own **`libc.a`** and **`libufs`** build products, plus an `-isystem ${PS5_PAYLOAD_SDK}/target/include` include root (`prospero-clang:74`).

**Default-linked (no extra flags):** `crt1.o`, `libc.a` (SDK's own), `-lSceLibcInternal`, `-lSceNet`, `-lkernel_web`.

**Opt-in (require an explicit `-lSce…` / `-l…` at link time):** every other Sony lib — e.g. `-lSceSsl -lSceHttp2 -lSceNetCtl -lSceSystemService -lufs …`. Example: `samples/http2_get/Makefile:28` →
`CFLAGS := -Wall -Werror -g -lSceNet -lSceSsl -lSceHttp2`.

Symbol resolution summary:

| Kind of symbol | Resolved from | Default-linked? |
|---|---|---|
| `ps5/*` kernel API | `crt1.o` | Yes |
| POSIX/BSD syscalls (`socket`, `open`, `pthread_*`, `mmap`) | `libkernel*` (`kernel_web`) | Yes (`-lkernel_web`) |
| Standard C library (`malloc`, stdio, string, math) | `SceLibcInternal` | Yes |
| SDK gap-fill libc (`dlopen`, `regcomp`, C11 `thrd_*`, …) | SDK's own `libc.a` | Yes |
| Sony `sceNet*` sockets | `libSceNet` | Yes (`-lSceNet`) |
| All other `sce*` / `mono_*` / `egl*` / `gl*` | their `libSce…` | No (opt-in `-lSce…`) |

### Legend: declared vs symbol-only

This is the single most important distinction for L2.

- **declared** = a C prototype exists in `include/freebsd/**` or `include/ps5/**`. **bindgen generates it directly** — no manual work.
- **symbol-only** = the symbol is exported by a `sce_stubs/*.c` stub (`asm .global NAME`) but **NO header/prototype exists in the SDK**. bindgen cannot help; to call it from Rust you must **hand-write the `extern "C"` prototype yourself** (exactly as `samples/http2_get/main.c` does), resolving argument types from PS5 libdoc / community headers.

To list a Sony lib's exported symbols:
`grep -oE '\.global [A-Za-z0-9_]+' sce_stubs/<lib>.c | awk '{print $2}' | sort -u`

---

## L2 focus areas at a glance

| Focus area | Recommended underlying API | declared / symbol-only | Provider / link flags | Wrapping recommendation |
|---|---|---|---|---|
| **File management** | POSIX/BSD (`open`/`read`/`stat`/`opendir`/`fopen`) | **declared** | `kernel_web` (syscalls) + `SceLibcInternal` (stdio); both default | bindgen directly off `fcntl.h`/`unistd.h`/`sys/stat.h`/`dirent.h`/`stdio.h`; no extra flags |
| **Networking** | Stack A: BSD sockets (`socket`/`bind`/`connect`) | **declared** | `kernel_web` + `SceLibcInternal` (`inet_*`, `getaddrinfo`); both default | Prefer Stack A — bindgen-able and default-linked. Use `sceNet*` only for PS5-specific features |
| | Stack B: Sony `sceNet*` / `sceNetCtl*` | **symbol-only** | `libSceNet` (default) / `libSceNetCtl` (`-lSceNetCtl`) | Hand-declare each extern; reach for it only for route/WLAN/AP/DHCP/NAT/state |
| **HTTP / TLS** | `sceHttp2*` + `sceSsl*` (+ `sceNet*` for init) | **symbol-only** | `libSceHttp2` (`-lSceHttp2`), `libSceSsl` (`-lSceSsl`), `libSceNet` (default) | Hand-declare all externs; copy the 16 known-good prototypes from `samples/http2_get/main.c`; init order Net→Ssl→Http2 |
| **Concurrency / threads** | POSIX `pthread_*` / `sem_*` / `sched_*`; C11 `thrd_*` | **declared** | `pthread_*`/`sem_*`/`sched_*` from `kernel_web`; C11 from SDK `libc.a`; both default | bindgen directly off `pthread.h`/`semaphore.h`/`sched.h`/`threads.h`; use Rust native atomics instead of `stdatomic.h` |

---

## 1. `ps5/*` kernel API (L1 — implemented)

These are the only APIs already wrapped (by `crates/ps5-sys`). All are **declared** in `include/ps5/*.h` and linked automatically via `crt1.o`. Provider: **`crt1.o`**.

Headers: `include/ps5/{kernel.h,klog.h,mdbg.h,nid.h,payload.h}` (~57 functions total).

### `kernel.h` — kernel read/write, process/credential introspection (44 functions)

- **Firmware / raw kernel memory:** `kernel_get_fw_version` (:56); `kernel_copyin` (:58), `kernel_copyout` (:59); typed setters `kernel_setlong`/`setint`/`setshort`/`setchar` (:61-64); typed getters `kernel_getlong`/`getint`/`getshort`/`getchar` (:66-69).
- **Process / fd lookup:** `kernel_get_proc` (:71), `kernel_get_proc_thread` (:72), `kernel_get_proc_ucred` (:73), `kernel_get_proc_filedesc` (:74), `kernel_get_proc_file` (:75).
- **Virtual-memory protection:** `kernel_get_vmem_protection` (:77), `kernel_set_vmem_protection` (:78), `kernel_mprotect` (:79).
- **Socket overlap:** `kernel_overlap_sockets` (:81).
- **Dynamic-linker introspection:** `kernel_dynlib_handle` (:83), `kernel_dynlib_dlsym` (:84), `kernel_dynlib_resolve` (:85), `kernel_dynlib_mapbase_addr` (:86), `kernel_dynlib_entry_addr` (:87), `kernel_dynlib_init_addr` (:88), `kernel_dynlib_fini_addr` (:89).
- **Credentials (authid / caps / attrs):** `kernel_get_ucred_authid` (:91), `kernel_set_ucred_authid` (:92), `kernel_get_ucred_caps` (:94), `kernel_set_ucred_caps` (:95), `kernel_get_ucred_attrs` (:97), `kernel_set_ucred_attrs` (:98).
- **QA flags:** `kernel_get_qaflags` (:100), `kernel_set_qaflags` (:101).
- **Root / jail vnodes:** `kernel_get_root_vnode` (:103), `kernel_get_proc_rootdir` (:105), `kernel_set_proc_rootdir` (:106), `kernel_get_proc_jaildir` (:108), `kernel_set_proc_jaildir` (:109).
- **uid/gid credentials:** `kernel_get_ucred_uid`/`set_ucred_uid` (:111-112), `…_ruid` (:114-115), `…_svuid` (:117-118), `…_rgid` (:120-121), `…_svgid` (:123-124).
- **Prison:** `kernel_get_ucred_prison` (:126), `kernel_set_ucred_prison` (:127).

### `klog.h` — kernel log output (3 functions)
`klog_printf` (:23), `klog_puts` (:24), `klog_perror` (:25).

### `mdbg.h` — cross-process (debugger) memory access (9 functions)
`mdbg_copyout` (:28), `mdbg_copyin` (:33); typed setters `mdbg_setchar`/`setshort`/`setint`/`setlong` (:38-41); typed getters `mdbg_getlong`/`getint`/`getshort`/`getchar` (:46-49).

### `nid.h` — symbol-name encoding (1 function)
`nid_encode(const char *sym, char buf[12])` (:26).

### `payload.h` — payload entry/exit (2 functions)
`payload_get_args` (:42), `payload_exit` (:51). The startup object `crt1.o` calls `_start` with a `payload_args_t*` (`crt/payload.h`, `crt/crt.c:197`).

---

## 2. File management  [FOCUS — not yet wrapped]

Use the POSIX/BSD + C-stdio surface. All entries are **declared** in `include/freebsd/**`, so bindgen generates them directly. Providers: syscall-level fd functions resolve from **`kernel_web`** (default); buffered stdio resolves from **`SceLibcInternal`** (default). No extra link flags.

### Low-level fd I/O (Provider: `kernel_web`)

| Function | Header:Line |
|---|---|
| `open` | `fcntl.h:318` |
| `openat` | `fcntl.h:325` |
| `creat` | `fcntl.h:319` |
| `read` | `unistd.h:357` |
| `write` | `unistd.h:370` |
| `pread` / `pwrite` | `unistd.h:420` / `:421` |
| `close` | `unistd.h:326` |
| `lseek` | `unistd.h:352` |
| `dup` / `dup2` | `unistd.h:328` / `:329` |
| `pipe` | `unistd.h:356` |
| `fsync` | `unistd.h:386` |
| `ftruncate` / `truncate` | `unistd.h:395` / `:426` |
| `unlink` | `unistd.h:369` |
| `rmdir` | `unistd.h:358` |
| `access` | `unistd.h:322` |
| `chdir` / `getcwd` | `unistd.h:324` / `:338` |
| `readlink` / `symlink` | `unistd.h:406` / `:446` |

### Metadata & directory ops (Provider: `kernel_web`)

| Function | Header:Line |
|---|---|
| `stat` | `sys/stat.h:348` |
| `fstat` | `sys/stat.h:334` |
| `lstat` | `sys/stat.h:340` |
| `fstatat` | `sys/stat.h:351` |
| `mkdir` | `sys/stat.h:342` |
| `mkfifo` | `sys/stat.h:343` |
| `chmod` / `fchmod` | `sys/stat.h:321` / `:326` |
| `umask` | `sys/stat.h:349` |
| `opendir` / `fdopendir` | `dirent.h:95` / `:96` |
| `readdir` / `readdir_r` | `dirent.h:98` / `:100` |
| `rewinddir` | `dirent.h:102` |
| `seekdir` / `telldir` | `dirent.h:114` / `:115` |
| `closedir` | `dirent.h:117` |

### Buffered stdio (Provider: `SceLibcInternal`)

| Function | Header:Line |
|---|---|
| `fopen` | `stdio.h:257` |
| `fclose` | `stdio.h:250` |
| `fread` / `fwrite` | `stdio.h:261` / `:267` |
| `fseek` / `ftell` | `stdio.h:264` / `:266` |
| `fgets` / `fputs` | `stdio.h:256` / `:260` |
| `fprintf` | `stdio.h:258` |
| `fflush` | `stdio.h:253` |
| `remove` / `rename` | `stdio.h:279` / `:280` |

> Extra filesystem helpers the SDK gap-fills in its own `libc.a` (also declared, default-linked) — directory traversal (`fts_*`, `nftw`, `scandir`, `dirfd`), pattern matching (`fnmatch`, `regcomp`/`regexec`/`regerror`/`regfree`), temp files (`mkstemp`/`mkdtemp`/`mktemp`/`tmpfile`), mount syscalls (`mount`/`nmount`/`unmount`/`getmntinfo`), and the raw FreeBSD syscall surface (`openat`/`fstatat`/`unlinkat`/`renameat`, `statfs`/`fstatfs`/`getfsstat`, …) — are catalogued in §7 "Other SDK-provided surfaces." Note there is **no** `glob.c`: `glob` is not gap-filled.

---

## 3. Networking  [FOCUS — not yet wrapped]

The SDK ships **two parallel network stacks**. Pick one per call site; do not mix fds between them.

| Stack | Kind | Default-linked? | Provider | bindgen-able? |
|---|---|---|---|---|
| **A. BSD sockets** (POSIX `socket`/`bind`/…) | **declared** (prototypes in `include/freebsd/**`) | **Yes** | `kernel_web` (syscalls) + `SceLibcInternal` (`inet_*`, `getaddrinfo`, `gethostbyname`) | **Yes** — bindgen generates directly |
| **B. Sony `sceNet*` / `sceNetCtl*`** | **symbol-only** (asm stubs, NO headers) | `libSceNet` **Yes**; `libSceNetCtl` **opt-in** (`-lSceNetCtl`) | `libSceNet.sprx` / `libSceNetCtl.sprx` | **No** — hand-write every `extern` prototype |

Default link gives you BSD sockets + `libSceNet` with no extra flags; `libSceNetCtl` and the TLS/HTTP libs need explicit `-lSce…`.

### A) BSD sockets — DECLARED (bindgen-able)

All prototypes are real C declarations under `include/freebsd/`. Line numbers from the cited header.

**Core socket calls — `sys/socket.h`** (Provider: `kernel_web` syscalls)

| Function | Signature | Line |
|---|---|---|
| `socket` | `int socket(int, int, int)` | 659 |
| `socketpair` | `int socketpair(int, int, int, int *)` | 660 |
| `bind` | `int bind(int, const struct sockaddr *, socklen_t)` | 628 |
| `listen` | `int listen(int, int)` | 638 |
| `accept` | `int accept(int, struct sockaddr * __restrict, socklen_t * __restrict)` | 627 |
| `connect` | `int connect(int, const struct sockaddr *, socklen_t)` | 629 |
| `send` | `ssize_t send(int, const void *, size_t, int)` | 647 |
| `recv` | `ssize_t recv(int, void *, size_t, int)` | 639 |
| `sendto` | `ssize_t sendto(int, const void *, …)` | 648 |
| `recvfrom` | `ssize_t recvfrom(int, void *, size_t, int, struct sockaddr * __restrict, socklen_t * __restrict)` | 640 |
| `sendmsg` | `ssize_t sendmsg(int, const struct msghdr *, int)` | 650 |
| `recvmsg` | `ssize_t recvmsg(int, struct msghdr *, int)` | 641 |
| `setsockopt` | `int setsockopt(int, int, int, const void *, socklen_t)` | 656 |
| `getsockopt` | `int getsockopt(int, int, int, void * __restrict, socklen_t * __restrict)` | 637 |
| `getsockname` | `int getsockname(int, struct sockaddr * __restrict, socklen_t * __restrict)` | 636 |
| `getpeername` | `int getpeername(int, struct sockaddr * __restrict, socklen_t * __restrict)` | 635 |
| `shutdown` | `int shutdown(int, int)` | 657 |

Supporting structs/consts: `netinet/in.h` (`struct in_addr` :81, `struct sockaddr_in` :95, `in_addr_t` :65, `INADDR_ANY` :46), `netinet6/in6.h` (`struct in6_addr` :95, `struct sockaddr_in6` :123, `in6addr_any` :209), `netinet/tcp.h` (TCP options, e.g. `TCP_NODELAY`).

**Address / byte-order helpers — `arpa/inet.h`** (Provider: `SceLibcInternal` for `inet_*`)

| Function | Signature | Line |
|---|---|---|
| `inet_addr` | `in_addr_t inet_addr(const char *)` | 145 |
| `inet_ntoa` | `char *inet_ntoa(struct in_addr)` | 146 |
| `inet_ntop` | `const char *inet_ntop(int, const void * __restrict, char * __restrict, socklen_t)` | 147 |
| `inet_pton` | `int inet_pton(int, const char * __restrict, void * __restrict)` | 149 |
| `inet_aton` | `int inet_aton(const char *, struct in_addr *)` | 152 |
| `inet_network` | `in_addr_t inet_network(const char *)` | 157 |

> `htons`/`htonl`/`ntohs`/`ntohl` (`arpa/inet.h:139-142`, macros → `__htonl` etc. at :170-173) are **preprocessor macros**, not linkable symbols. They expand to `static __inline __bswap16_var`/`__bswap32_var` (`x86/endian.h:92-118`) — compile-time/inline. **bindgen will NOT emit them**; reimplement on the Rust side (`u16::to_be`/`u32::to_be` or a `const fn`).

**Name resolution — `netdb.h`** (Provider: `SceLibcInternal`)

| Function | Signature | Line |
|---|---|---|
| `getaddrinfo` | `int getaddrinfo(const char *, const char *, …)` | 249 |
| `getnameinfo` | `int getnameinfo(const struct sockaddr *, socklen_t, char *, …)` | 251 |
| `freeaddrinfo` | `void freeaddrinfo(struct addrinfo *)` | 253 |
| `gai_strerror` | `const char *gai_strerror(int)` | 254 |
| `gethostbyname` | `struct hostent *gethostbyname(const char *)` | 233 |
| `gethostbyaddr` | `struct hostent *gethostbyaddr(const void *, socklen_t, int)` | 232 |
| `getservbyname` | `struct servent *getservbyname(const char *, const char *)` | 242 |
| `getservbyport` | `struct servent *getservbyport(int, const char *)` | 243 |
| `getprotobyname` | `struct protoent *getprotobyname(const char *)` | 239 |

**Polling / multiplexing / fd control** (Provider: `kernel_web` syscalls)

| Function | Signature | Header:Line |
|---|---|---|
| `poll` | `int poll(struct pollfd _pfd[], nfds_t _nfds, int _timeout)` | `sys/poll.h:112` (via `poll.h` symlink) |
| `select` | `int select(int, fd_set *, fd_set *, fd_set *, struct timeval *)` | `sys/select.h:103` |
| `pselect` | `int pselect(int, fd_set *__restrict, …)` | `sys/select.h:98` |
| `ioctl` | `int ioctl(int, unsigned long, ...)` | `sys/ioccom.h:79` (pulled in by `sys/ioctl.h`) |
| `fcntl` | `int fcntl(int, int, ...)` — set non-blocking with `O_NONBLOCK` (`0x0004`) | `fcntl.h:320` / flag at `fcntl.h:90` |

**Interface enumeration — `net/if.h`** (Provider: `kernel_web`)
`if_nametoindex` (:606), `if_indextoname` (:604), `if_nameindex` (:605), `if_freenameindex` (:603).

### B) Sony `sceNet*` / `sceNetCtl*` — SYMBOL-ONLY (NO headers)

Exported by asm stubs only — **no prototype anywhere in the SDK**. To call from Rust you **must hand-write the `extern "C"` declaration yourself** (as `samples/http2_get/main.c` does). bindgen cannot help; resolve each signature from libdocs / community headers.

#### `libSceNet` — **220 exported symbols** (default-linked via `-lSceNet`)
Source: `sce_stubs/libSceNet.c`. Recipe: `grep -oE '\.global [A-Za-z0-9_]+' sce_stubs/libSceNet.c | awk '{print $2}' | sort -u`.

- **Lifecycle / pools / memory:** `sceNetInit`, `sceNetInitParam`, `sceNetTerm`, `sceNetPoolCreate`, `sceNetPoolDestroy`, `sceNetMemoryAllocate`, `sceNetMemoryFree`, `sceNetGetMemoryPoolStats`, `sceNetDbgInit`, `sce_net_dummy`
- **Socket ops:** `sceNetSocket`, `sceNetSocketAbort`, `sceNetSocketClose`, `sceNetSocketInternal`, `sceNetAccept`, `sceNetBind`, `sceNetConnect`, `sceNetListen`, `sceNetShutdown`, `sceNetSend`, `sceNetSendto`, `sceNetSendmsg`, `sceNetRecv`, `sceNetRecvfrom`, `sceNetRecvmsg`, `sceNetSetsockopt`, `sceNetGetsockopt`, `sceNetGetsockname`, `sceNetGetpeername`, `sceNetGetSockInfo`, `sceNetGetSockInfo6`, `sceNetIoctl`, `sceNetSysctl`, `sceNetControl`, `sceNetErrnoLoc`
- **Epoll:** `sceNetEpollCreate`, `sceNetEpollControl`, `sceNetEpollWait`, `sceNetEpollDestroy`, `sceNetEpollAbort`
- **Resolver / DNS:** `sceNetResolverCreate`, `sceNetResolverDestroy`, `sceNetResolverAbort`, `sceNetResolverGetError`, `sceNetResolverConnect`, `sceNetResolverConnectCreate`, `sceNetResolverConnectDestroy`, `sceNetResolverConnectAbort`, `sceNetResolverStartAton`, `sceNetResolverStartAton6`, `sceNetResolverStartNtoa`, `sceNetResolverStartNtoa6`, `sceNetResolverStartNtoaMultipleRecords`, `sceNetResolverStartNtoaMultipleRecordsEx`; DNS info: `sceNetGetDnsInfo`, `sceNetGetDns6Info`, `sceNetSetDnsInfo`, `sceNetSetDns6Info`, `sceNetSetDnsInfoToKernel`, `sceNetSetDns6InfoToKernel`, `sceNetClearDnsCache`
- **Byte-order / inet helpers:** `sceNetHtons`, `sceNetHtonl`, `sceNetHtonll`, `sceNetNtohs`, `sceNetNtohl`, `sceNetNtohll`, `sceNetInetPton`, `sceNetInetPtonEx`, `sceNetInetPtonWithScopeId`, `sceNetInetNtop`, `sceNetInetNtopWithScopeId`, `sceNetEtherNtostr`, `sceNetEtherStrton`, `sceNetGetMacAddress`, `sceNetGetRandom`, `sceNetGetSystemTime`, `sceNetUsleep`
- **Config / routing / interfaces (largest cluster):** `sceNetConfigAddArp`, `sceNetConfigAddArpWithInterface`, `sceNetConfigAddIfaddr`, `sceNetConfigAddMRoute`, `sceNetConfigAddRoute`, `sceNetConfigAddRoute6`, `sceNetConfigAddRouteWithInterface`, `sceNetConfigCleanUpAllInterfaces`, `sceNetConfigDelArp`, `sceNetConfigDelArpWithInterface`, `sceNetConfigDelDefaultRoute`, `sceNetConfigDelDefaultRoute6`, `sceNetConfigDelDefaultRouteDev`, `sceNetConfigDelIfaddr`, `sceNetConfigDelIfaddr6`, `sceNetConfigDelMRoute`, `sceNetConfigDelRoute`, `sceNetConfigDelRoute6`, `sceNetConfigDownInterface`, `sceNetConfigEtherGetLinkMode`, `sceNetConfigEtherPostPlugInOutEvent`, `sceNetConfigEtherSetLinkMode`, `sceNetConfigFlushRoute`, `sceNetConfigGetDefaultRoute`, `sceNetConfigGetDefaultRoute6`, `sceNetConfigGetIfaddr`, `sceNetConfigGetIfaddr6`, `sceNetConfigRoutingShowRoutingConfig`, `sceNetConfigRoutingShowtCtlVar`, `sceNetConfigRoutingStart`, `sceNetConfigRoutingStop`, `sceNetConfigSetDefaultRoute`, `sceNetConfigSetDefaultRoute6`, `sceNetConfigSetDefaultRouteDev`, `sceNetConfigSetDefaultScope`, `sceNetConfigSetIfFlags`, `sceNetConfigSetIfLinkLocalAddr6`, `sceNetConfigSetIfaddr`, `sceNetConfigSetIfaddr6`, `sceNetConfigSetIfaddr6WithFlags`, `sceNetConfigSetIfmtu`, `sceNetConfigUnsetIfFlags`, `sceNetConfigUpInterface`, `sceNetConfigUpInterfaceWithFlags`
- **Config / WLAN + ad-hoc (`sceNetConfigWlan*`):** `sceNetConfigWlanAdhocClearWakeOnWlan`, `sceNetConfigWlanAdhocCreate`, `sceNetConfigWlanAdhocGetWakeOnWlanInfo`, `sceNetConfigWlanAdhocJoin`, `sceNetConfigWlanAdhocLeave`, `sceNetConfigWlanAdhocPspEmuClearWakeOnWlan`, `sceNetConfigWlanAdhocPspEmuGetWakeOnWlanInfo`, `sceNetConfigWlanAdhocPspEmuSetWakeOnWlan`, `sceNetConfigWlanAdhocScanJoin`, `sceNetConfigWlanAdhocSetExtInfoElement`, `sceNetConfigWlanAdhocSetWakeOnWlan`, `sceNetConfigWlanApStart`, `sceNetConfigWlanApStop`, `sceNetConfigWlanBackgroundScanQuery`, `sceNetConfigWlanBackgroundScanStart`, `sceNetConfigWlanBackgroundScanStop`, `sceNetConfigWlanDiagGetDeviceInfo`, `sceNetConfigWlanDiagSetAntenna`, `sceNetConfigWlanDiagSetTxFixedRate`, `sceNetConfigWlanGetDeviceConfig`, `sceNetConfigWlanInfraGetRssiInfo`, `sceNetConfigWlanInfraLeave`, `sceNetConfigWlanInfraScanJoin`, `sceNetConfigWlanScan`, `sceNetConfigWlanSetDeviceConfig`
- **Routing info / show / netstat:** `sceNetAllocateAllRouteInfo`, `sceNetFreeAllRouteInfo`, `sceNetGetRouteInfo`, `sceNetGetArpInfo`, `sceNetShowRoute`, `sceNetShowRoute6`, `sceNetShowRouteForBuffer`, `sceNetShowRouteWithMemory`, `sceNetShowRoute6ForBuffer`, `sceNetShowRoute6WithMemory`, `sceNetShowIfconfig`, `sceNetShowIfconfigForBuffer`, `sceNetShowIfconfigWithMemory`, `sceNetShowNetstat`, `sceNetShowNetstatEx`, `sceNetShowNetstatForBuffer`, `sceNetShowNetstatExForBuffer`, `sceNetShowNetstatWithMemory`, `sceNetShowPolicy`, `sceNetShowPolicyWithMemory`
- **DHCP / PPPoE / addr-config / autoip:** `sceNetDhcpStart`, `sceNetDhcpStop`, `sceNetDhcpGetInfo`, `sceNetDhcpGetInfoEx`, `sceNetDhcpGetAutoipInfo`, `sceNetDhcpdStart`, `sceNetDhcpdStop`, `sceNetPppoeStart`, `sceNetPppoeStop`, `sceNetAddrConfig6Start`, `sceNetAddrConfig6Stop`, `sceNetAddrConfig6GetInfo`, `sceNetDuplicateIpStart`, `sceNetDuplicateIpStop`
- **Interface list / name / stats:** `sceNetGetIfList`, `sceNetGetIfListOnce`, `sceNetGetIfName`, `sceNetGetIfnameNumList`, `sceNetGetNameToIndex`, `sceNetGetInterfaceStats`, `sceNetGetStatisticsInfo`, `sceNetGetStatisticsInfoInternal`
- **Bandwidth control:** `sceNetBandwidthControlGetDataTraffic`, `sceNetBandwidthControlGetDefaultParam`, `sceNetBandwidthControlSetDefaultParam`
- **Emulation:** `sceNetEmulationGet`, `sceNetEmulationSet`, `sceNetEmulationDebugSettingsSet`
- **Event callbacks / sync / threads:** `sceNetEventCallbackCreate`, `sceNetEventCallbackDestroy`, `sceNetEventCallbackGetError`, `sceNetEventCallbackWaitCB`, `sceNetSyncCreate`, `sceNetSyncDestroy`, `sceNetSyncGet`, `sceNetSyncSignal`, `sceNetSyncWait`, `sceNetThreadCreate`, `sceNetThreadExit`, `sceNetThreadJoin`
- **Packet dump / info dump:** `sceNetDumpCreate`, `sceNetDumpDestroy`, `sceNetDumpRead`, `sceNetDumpAbort`, `sceNetInfoDumpStart`, `sceNetInfoDumpStop`
- **Exported data symbols (addresses, not functions):** `in6addr_any`, `in6addr_loopback`, `sce_net_in6addr_any`, `sce_net_in6addr_loopback`, `sce_net_in6addr_linklocal_allnodes`, `sce_net_in6addr_linklocal_allrouters`, `sce_net_in6addr_nodelocal_allnodes`

#### `libSceNetCtl` — **95 exported symbols** (opt-in: add `-lSceNetCtl`)
Source: `sce_stubs/libSceNetCtl.c`.

- **Public lifecycle / info / state (realistically callable):** `sceNetCtlInit`, `sceNetCtlTerm`, `sceNetCtlGetInfo`, `sceNetCtlGetInfoV6`, `sceNetCtlGetState`, `sceNetCtlGetStateV6`, `sceNetCtlGetResult`, `sceNetCtlGetResultV6`, `sceNetCtlGetNatInfo`, `sceNetCtlGetIfStat`, `sceNetCtlGetWifiType`, `sceNetCtlGetEtherLinkMode`
- **Public callback management:** `sceNetCtlCheckCallback`, `sceNetCtlRegisterCallback`, `sceNetCtlUnregisterCallback`, `sceNetCtlRegisterCallbackV6`, `sceNetCtlUnregisterCallbackV6`
- **NpToolkit / Lib callback variants:** `sceNetCtlCheckCallbackForNpToolkit`, `sceNetCtlClearEventForNpToolkit`, `sceNetCtlRegisterCallbackForNpToolkit`, `sceNetCtlUnregisterCallbackForNpToolkit`, `sceNetCtlCheckCallbackForLibIpcInt`, `sceNetCtlClearEventForLibIpcInt`, `sceNetCtlRegisterCallbackForLibIpcInt`, `sceNetCtlUnregisterCallbackForLibIpcInt`
- **`*IpcInt` internal IPC variants (system-process oriented — generally not for homebrew):** `sceNetCtlCheckCallbackIpcInt`, `sceNetCtlClearEventIpcInt`, `sceNetCtlConnectIpcInt`, `sceNetCtlConnectConfIpcInt`, `sceNetCtlConnectWithRetryIpcInt`, `sceNetCtlDisconnectIpcInt`, `sceNetCtlGetInfoIpcInt`, `sceNetCtlGetInfoV6IpcInt`, `sceNetCtlGetResultIpcInt`, `sceNetCtlGetResultV6IpcInt`, `sceNetCtlGetStateIpcInt`, `sceNetCtlGetState2IpcInt`, `sceNetCtlGetStateV6IpcInt`, `sceNetCtlGetNatInfoIpcInt`, `sceNetCtlGetBandwidthInfoIpcInt`, `sceNetCtlGetConnectionElapsedTimeIpcInt`, `sceNetCtlGetNetEvConfigInfoIpcInt`, `sceNetCtlRegisterCallbackIpcInt`, `sceNetCtlRegisterCallbackV6IpcInt`, `sceNetCtlUnregisterCallbackIpcInt`, `sceNetCtlUnregisterCallbackV6IpcInt`, `sceNetCtlScanIpcInt`, `sceNetCtlEnableBandwidthManagementIpcInt`, `sceNetCtlDisableBandwidthManagementIpcInt`, `sceNetCtlIsBandwidthManagementEnabledIpcInt`, `sceNetCtlSetErrorNotificationEnabledIpcInt`, `sceNetCtlSetStunWithPaddingFlagIpcInt`, `sceNetCtlUnsetStunWithPaddingFlagIpcInt`
- **Scan-info IPC variants:** `sceNetCtlGetScanInfoBssidIpcInt`, `sceNetCtlGetScanInfoByBssidIpcInt`, `sceNetCtlGetScanInfoBssidForSsidListScanIpcInt`, `sceNetCtlGetScanInfoForSsidListScanIpcInt`, `sceNetCtlGetScanInfoForSsidScanIpcInt`
- **AP cluster (`sceNetCtlAp*`):** `sceNetCtlApInit`, `sceNetCtlApTerm`, `sceNetCtlApGetState`, `sceNetCtlApGetInfo`, `sceNetCtlApGetConnectInfo`, `sceNetCtlApGetResult`, `sceNetCtlApCheckCallback`, `sceNetCtlApClearEvent`, `sceNetCtlApRegisterCallback`, `sceNetCtlApUnregisterCallback`, `sceNetCtlApAppInitWpaKey`, `sceNetCtlApAppInitWpaKeyForQa`, `sceNetCtlApAppStartWithRetry`, `sceNetCtlApAppStartWithRetryPid`, `sceNetCtlApRestart`, `sceNetCtlApStop`, `sceNetCtlApCpStart`, `sceNetCtlApCpStop`
- **AP-RP sub-cluster (`sceNetCtlApRp*`):** `sceNetCtlApRpStart`, `sceNetCtlApRpStartConf`, `sceNetCtlApRpStartWithRetry`, `sceNetCtlApRpStop`, `sceNetCtlApRpGetState`, `sceNetCtlApRpGetInfo`, `sceNetCtlApRpGetResult`, `sceNetCtlApRpCheckCallback`, `sceNetCtlApRpClearEvent`, `sceNetCtlApRpRegisterCallback`, `sceNetCtlApRpUnregisterCallback`
- **BWE (bandwidth-estimation / internet-connection-test) cluster (`sceNetBwe*`):** `sceNetBweCheckCallbackIpcInt`, `sceNetBweClearEventIpcInt`, `sceNetBweGetInfoIpcInt`, `sceNetBweRegisterCallbackIpcInt`, `sceNetBweUnregisterCallbackIpcInt`, `sceNetBweStartInternetConnectionTestIpcInt`, `sceNetBweStartInternetConnectionTestBandwidthTestIpcInt`, `sceNetBweFinishInternetConnectionTestIpcInt`, `sceNetBweSetInternetConnectionTestResultIpcInt`

> **Hand-written prototype requirement (both Sony libs):** `sce_stubs/*.c` emit only `.global <NAME>` with no `.h` prototype, so bindgen has nothing to parse. Mirror `samples/http2_get/main.c`: write each prototype by hand in an `extern "C" { … }` block (resolving true argument types from PS5 libdoc/community headers), then link with the right flag (`libSceNet` automatic; `libSceNetCtl` needs `-lSceNetCtl`). For ordinary TCP/UDP homebrew, prefer **Stack A (BSD sockets)** — declared, bindgen-able, default-linked — and reach for `sceNet*`/`sceNetCtl*` only for PS5-specific features (interface/route config, WLAN/AP, DHCP, NAT info, connection state/events).

---

## 4. HTTP & TLS  [FOCUS — not yet wrapped]

> **Status: symbol-only across the board.** No lib ships a header (`grep -rilE 'sceHttp2|sceHttp|sceSsl' include/` returns nothing). bindgen **cannot** generate these; to call them from Rust you must hand-write each `extern "C"` prototype, exactly as `samples/http2_get/main.c` does.
>
> **Provider / linkage (all opt-in — NOT default-linked):** symbols come from `sce_stubs/*.c` stubs. Pass `-lSceNet -lSceSsl -lSceHttp2` explicitly (`samples/http2_get/Makefile:28`). `prospero-clang` links none of them by default.
>
> **Dependency / init order** (`samples/http2_get/main.c:55-89`): **Net → Ssl → Http2**. `sceNetInit()` → `sceNetPoolCreate()` (net mem-pool id) → `sceSslInit()` (ssl ctx id) → `sceHttp2Init(netMemId, sslCtxId, …)` (consumes both ids). Teardown reverses (`main.c:127-158`): delete request/template, then `sceHttp2Term` → `sceSslTerm` → `sceNetPoolDestroy`.

### Known-good hand-declared prototypes (reuse verbatim in L2)

These are the **exact** prototypes from `samples/http2_get/main.c:24-42` — copy verbatim into Rust `extern "C"` blocks. (`sceNetInit`/`sceNetPoolCreate`/`sceNetPoolDestroy` are from `libSceNet`; included because the init sequence requires them.) Only these 16 are SDK-attested; every other symbol below has **no signature anywhere in the SDK** and must be reversed/guessed before calling.

```c
/* libSceNet (-lSceNet) — main.c:24-26 */
int sceNetInit();
int sceNetPoolCreate(const char*, int, int);
int sceNetPoolDestroy(int);

/* libSceSsl (-lSceSsl) — main.c:28-29 */
int sceSslInit(size_t);
int sceSslTerm(int);

/* libSceHttp2 (-lSceHttp2) — main.c:31-42 */
int sceHttp2Init(int, int, size_t, int);
int sceHttp2Term(int);

int sceHttp2CreateTemplate(int, const char*, int, int);
int sceHttp2DeleteTemplate(int);

int sceHttp2CreateRequestWithURL(int, const char*, const char*, uint64_t);
int sceHttp2DeleteRequest(int);

int sceHttp2SendRequest(int, const void*, size_t);
int sceHttp2GetStatusCode(int, int*);
int sceHttp2ReadData(int, void *, size_t);
```

### `sce_stubs/libSceHttp2.c` — modern HTTP/2 client (`-lSceHttp2`, opt-in) — 55 `sceHttp2*` symbols

Used by the `http2_get` sample. All sorted symbols:
`sceHttp2AbortRequest`, `sceHttp2AddCookie`, `sceHttp2AddRequestHeader`, `sceHttp2AuthCacheFlush`, `sceHttp2CookieExport`, `sceHttp2CookieFlush`, `sceHttp2CookieImport`, `sceHttp2CreateCookieBox`, `sceHttp2CreateRequestWithURL`, `sceHttp2CreateTemplate`, `sceHttp2DeleteCookieBox`, `sceHttp2DeleteRequest`, `sceHttp2DeleteTemplate`, `sceHttp2GetAllResponseHeaders`, `sceHttp2GetAuthEnabled`, `sceHttp2GetAutoRedirect`, `sceHttp2GetCookie`, `sceHttp2GetCookieBox`, `sceHttp2GetCookieStats`, `sceHttp2GetMemoryPoolStats`, `sceHttp2GetResponseContentLength`, `sceHttp2GetStatusCode`, `sceHttp2Init`, `sceHttp2ReadData`, `sceHttp2ReadDataAsync`, `sceHttp2RedirectCacheFlush`, `sceHttp2RemoveRequestHeader`, `sceHttp2SendRequest`, `sceHttp2SendRequestAsync`, `sceHttp2SetAuthEnabled`, `sceHttp2SetAuthInfoCallback`, `sceHttp2SetAutoRedirect`, `sceHttp2SetConnectTimeOut`, `sceHttp2SetConnectionWaitTimeOut`, `sceHttp2SetCookieBox`, `sceHttp2SetCookieMaxNum`, `sceHttp2SetCookieMaxNumPerDomain`, `sceHttp2SetCookieMaxSize`, `sceHttp2SetCookieRecvCallback`, `sceHttp2SetCookieSendCallback`, `sceHttp2SetInflateGZIPEnabled`, `sceHttp2SetMinSslVersion`, `sceHttp2SetPreSendCallback`, `sceHttp2SetRecvTimeOut`, `sceHttp2SetRedirectCallback`, `sceHttp2SetRequestContentLength`, `sceHttp2SetResolveRetry`, `sceHttp2SetResolveTimeOut`, `sceHttp2SetSendTimeOut`, `sceHttp2SetSslCallback`, `sceHttp2SetTimeOut`, `sceHttp2SslDisableOption`, `sceHttp2SslEnableOption`, `sceHttp2Term`, `sceHttp2WaitAsync`.
> The stub also exports `_Z5dummyv` — a C++-mangled `dummy()` placeholder, not a usable API.

### `sce_stubs/libSceHttp.c` — legacy HTTP/1.x client (`-lSceHttp`, opt-in) — 115 symbols

Symbol-only. **Not used by any sample** — listed for completeness; prefer Http2. Needs its own `-lSceHttp` (not in the default link set).

Core `sceHttp*`:
`sceHttpAbortRequest`, `sceHttpAbortRequestForce`, `sceHttpAbortWaitRequest`, `sceHttpAddCookie`, `sceHttpAddQuery`, `sceHttpAddRequestHeader`, `sceHttpAddRequestHeaderRaw`, `sceHttpAuthCacheExport`, `sceHttpAuthCacheFlush`, `sceHttpAuthCacheImport`, `sceHttpCacheRedirectedConnectionEnabled`, `sceHttpCookieExport`, `sceHttpCookieFlush`, `sceHttpCookieImport`, `sceHttpCreateConnection`, `sceHttpCreateConnectionWithURL`, `sceHttpCreateEpoll`, `sceHttpCreateRequest`, `sceHttpCreateRequest2`, `sceHttpCreateRequestWithURL`, `sceHttpCreateRequestWithURL2`, `sceHttpCreateTemplate`, `sceHttpDbgEnableProfile`, `sceHttpDbgGetConnectionStat`, `sceHttpDbgGetRequestStat`, `sceHttpDbgSetPrintf`, `sceHttpDbgShowConnectionStat`, `sceHttpDbgShowMemoryPoolStat`, `sceHttpDbgShowRequestStat`, `sceHttpDbgShowStat`, `sceHttpDeleteConnection`, `sceHttpDeleteRequest`, `sceHttpDeleteTemplate`, `sceHttpDestroyEpoll`, `sceHttpGetAcceptEncodingGZIPEnabled`, `sceHttpGetAllResponseHeaders`, `sceHttpGetAuthEnabled`, `sceHttpGetAutoRedirect`, `sceHttpGetConnectionStat`, `sceHttpGetCookie`, `sceHttpGetCookieEnabled`, `sceHttpGetCookieStats`, `sceHttpGetEpoll`, `sceHttpGetEpollId`, `sceHttpGetLastErrno`, `sceHttpGetMemoryPoolStats`, `sceHttpGetNonblock`, `sceHttpGetRegisteredCtxIds`, `sceHttpGetResponseContentLength`, `sceHttpGetStatusCode`, `sceHttpInit`, `sceHttpParseResponseHeader`, `sceHttpParseStatusLine`, `sceHttpReadData`, `sceHttpRedirectCacheFlush`, `sceHttpRemoveRequestHeader`, `sceHttpRequestGetAllHeaders`, `sceHttpSendRequest`, `sceHttpSetAcceptEncodingGZIPEnabled`, `sceHttpSetAuthEnabled`, `sceHttpSetAuthInfoCallback`, `sceHttpSetAutoRedirect`, `sceHttpSetChunkedTransferEnabled`, `sceHttpSetConnectTimeOut`, `sceHttpSetCookieEnabled`, `sceHttpSetCookieMaxNum`, `sceHttpSetCookieMaxNumPerDomain`, `sceHttpSetCookieMaxSize`, `sceHttpSetCookieRecvCallback`, `sceHttpSetCookieSendCallback`, `sceHttpSetCookieTotalMaxSize`, `sceHttpSetDefaultAcceptEncodingGZIPEnabled`, `sceHttpSetDelayBuildRequestEnabled`, `sceHttpSetEpoll`, `sceHttpSetEpollId`, `sceHttpSetHttp09Enabled`, `sceHttpSetInflateGZIPEnabled`, `sceHttpSetNonblock`, `sceHttpSetPolicyOption`, `sceHttpSetPriorityOption`, `sceHttpSetProxy`, `sceHttpSetRecvBlockSize`, `sceHttpSetRecvTimeOut`, `sceHttpSetRedirectCallback`, `sceHttpSetRequestContentLength`, `sceHttpSetRequestStatusCallback`, `sceHttpSetResolveRetry`, `sceHttpSetResolveTimeOut`, `sceHttpSetResponseHeaderMaxSize`, `sceHttpSetSendTimeOut`, `sceHttpSetSocketCreationCallback`, `sceHttpTerm`, `sceHttpTryGetNonblock`, `sceHttpTrySetNonblock`, `sceHttpUnsetEpoll`, `sceHttpWaitRequest`.

URI helpers (same lib): `sceHttpUriBuild`, `sceHttpUriCopy`, `sceHttpUriEscape`, `sceHttpUriMerge`, `sceHttpUriParse`, `sceHttpUriSweepPath`, `sceHttpUriUnescape`.

HTTPS helpers (same lib, `sceHttps*`): `sceHttpsDisableOption`, `sceHttpsDisableOptionPrivate`, `sceHttpsEnableOption`, `sceHttpsEnableOptionPrivate`, `sceHttpsFreeCaList`, `sceHttpsGetCaList`, `sceHttpsGetSslError`, `sceHttpsLoadCert`, `sceHttpsSetMinSslVersion`, `sceHttpsSetSslCallback`, `sceHttpsSetSslVersion`, `sceHttpsUnloadCert`.

### `sce_stubs/libSceSsl.c` — TLS (`-lSceSsl`, opt-in) — 54 symbols

Only `sceSslInit`/`sceSslTerm` are exercised by the sample (verbatim prototypes above); the rest need hand-written prototypes. Symbols:
`sceSslCheckRecvPending`, `sceSslClose`, `sceSslConnect`, `sceSslCreateConnection`, `sceSslCreateSslConnection`, `sceSslDeleteConnection`, `sceSslDeleteSslConnection`, `sceSslDisableOption`, `sceSslDisableOptionInternal`, `sceSslDisableOptionInternalInsecure`, `sceSslDisableVerifyOption`, `sceSslEnableOption`, `sceSslEnableOptionInternal`, `sceSslEnableVerifyOption`, `sceSslFreeCaCerts`, `sceSslFreeCaList`, `sceSslFreeSslCertName`, `sceSslGetAlpnSelected`, `sceSslGetCaCerts`, `sceSslGetCaList`, `sceSslGetFingerprint`, `sceSslGetIssuerName`, `sceSslGetMemoryPoolStats`, `sceSslGetNameEntryCount`, `sceSslGetNameEntryInfo`, `sceSslGetNanoSSLModuleId`, `sceSslGetNotAfter`, `sceSslGetNotBefore`, `sceSslGetPeerCert`, `sceSslGetPem`, `sceSslGetSerialNumber`, `sceSslGetSslError`, `sceSslGetSubjectName`, `sceSslInit`, `sceSslLoadCert`, `sceSslLoadRootCACert`, `sceSslRead`, `sceSslRecv`, `sceSslReuseConnection`, `sceSslSend`, `sceSslSetAlpn`, `sceSslSetMinSslVersion`, `sceSslSetSslVersion`, `sceSslSetVerifyCallback`, `sceSslTerm`, `sceSslUnloadCert`, `sceSslWrite`.

> The same stub also exports lower-level NanoSSL/Mocana crypto primitives (not `sce*`-prefixed, symbol-only, no SDK prototype): `CA_MGMT_extractKeyBlobEx`, `CA_MGMT_extractPublicKeyInfo`, `CA_MGMT_freeKeyBlob`, `CRYPTO_initAsymmetricKey`, `CRYPTO_uninitAsymmetricKey`, `RSA_verifySignature`, `VLONG_freeVlongQueue`.

---

## 5. Concurrency & threads  [FOCUS — not yet wrapped]

All entries below are **declared** (a C prototype exists in `include/freebsd/`), so bindgen can generate them directly. Two provider layers:

- **POSIX/BSD layer** (`pthread_*`, `sem_*`, `sched_*`): symbols exported by **libkernel** (default `-lkernel_web`). The bare POSIX names appear as `.global` exports in `sce_stubs/libkernel_web.c`; the concurrency export set is byte-for-byte identical across `libkernel.c`/`libkernel_sys.c`/`libkernel_web.c` (174 symbols each). Linkage is satisfied automatically — no opt-in flag.
- **C11 thread layer** (`thrd_*`, `mtx_*`, `cnd_*`, `tss_*`, `call_once`): **not** exported by any `sce_stubs/libkernel*.c`. These are thin wrappers compiled into the SDK's own **`libc.a`** (`libc/Makefile:42,49`), each calling down to the underlying `pthread_*` symbol. Sources: `libc/{thrd,mtx,cnd,tss,call_once}.c`.

Opaque handle types are forward-declared in `include/freebsd/sys/_pthreadtypes.h` (`pthread_t`, `pthread_mutex_t`, `pthread_cond_t`, `pthread_rwlock_t`, `pthread_barrier_t`, `pthread_spinlock_t`, their `*attr_t` variants, `pthread_key_t` = `int`, `pthread_once_t` = `struct pthread_once`). C11 aliases (`thrd_t`, `mtx_t`, `cnd_t`, `tss_t`, `once_flag`) at `include/freebsd/threads.h:43-54`.

### Threads (`pthread_*`) — Provider: libkernel (`kernel_web`), declared in `include/freebsd/pthread.h`

| Function | Header line |
|---|---|
| `pthread_create` | pthread.h:210 |
| `pthread_join` | pthread.h:218 |
| `pthread_detach` | pthread.h:213 |
| `pthread_exit` | pthread.h:215 |
| `pthread_self` | pthread.h:280 |
| `pthread_equal` | pthread.h:214 |
| `pthread_once` | pthread.h:246 |
| `pthread_atfork` | pthread.h:149 |
| `pthread_cancel` / `setcancelstate` / `setcanceltype` / `testcancel` | pthread.h:293-296 |
| `pthread_getcpuclockid` | pthread.h:217 |
| `pthread_getprio` / `setprio` / `yield` (BSD-visible) | pthread.h:299-301 |
| `pthread_getconcurrency` / `setconcurrency` (XSI) | pthread.h:340-341 |
| `pthread_getschedparam` / `setschedparam` | pthread.h:335-338 |
| `__pthread_cleanup_push_imp` / `__pthread_cleanup_pop_imp` (back `pthread_cleanup_push/pop`, :178-189) | pthread.h:344-346 |

**Thread attributes (`pthread_attr_*`):** `pthread.h:150-167` and `:322-334` — `init`, `destroy`, `get/setstack`, `get/setstacksize`, `get/setstackaddr`, `get/setguardsize`, `get/setdetachstate`, `get/setinheritsched`, `get/setschedparam`, `get/setschedpolicy`, `get/setscope`.

### Mutexes — Provider: libkernel, declared `pthread.h:221-320`
`pthread_mutex_init` (:234), `destroy` (:232), `lock` (:237), `trylock` (:239), `timedlock` (:241), `unlock` (:244), `consistent` (:230), `get/setprioceiling` (:308-311). Attrs: `init/destroy` (:221-222), `get/setpshared`, `get/settype` (:223-229), `get/setprioceiling` (:304-307), `get/setprotocol` (:313-316), `get/setrobust` (:318-320). `PTHREAD_MUTEX_INITIALIZER` and type enum at pthread.h:100 / :128-134.

### Condition variables — Provider: libkernel, declared `pthread.h:191-209`
`pthread_cond_init` (:200), `destroy` (:199), `wait` (:207), `timedwait` (:203), `signal` (:202), `broadcast` (:198). Attrs: `init/destroy` (:191/:195), `get/setclock` (:192/:196), `get/setpshared` (:194/:197). `PTHREAD_COND_INITIALIZER` at :102.

### Read/write locks — Provider: libkernel, declared `pthread.h:247-279`
`pthread_rwlock_init` (:249), `destroy` (:247), `rdlock` (:252), `wrlock` (:268), `tryrdlock` (:262), `trywrlock` (:264), `timedrdlock` (:254), `timedwrlock` (:258), `unlock` (:266). Attrs: `init/destroy` (:276/:270), `get/setpshared` (:273/:279), `get/setkind_np` (:271/:277). `PTHREAD_RWLOCK_INITIALIZER` at :103.

### Barriers & spinlocks — Provider: libkernel, declared `pthread.h`
Barriers: `pthread_barrier_init` (:169), `destroy` (:168), `wait` (:171); `pthread_barrierattr_init/destroy` (:175/:172), `get/setpshared` (:173/:176). `PTHREAD_BARRIER_SERIAL_THREAD` at :56.
Spinlocks: `pthread_spin_init` (:283), `destroy` (:285), `lock` (:287), `trylock` (:289), `unlock` (:291).

### Thread-local storage — Provider: libkernel, declared `pthread.h`
`pthread_key_create` (:219), `pthread_key_delete` (:220), `pthread_getspecific` (:216), `pthread_setspecific` (:281). `PTHREAD_KEYS_MAX` (:53), `PTHREAD_DESTRUCTOR_ITERATIONS` (:52).

### Non-portable extensions — Provider: libkernel, declared `pthread_np.h:45-72` (needs `<sys/cpuset.h>`)
`pthread_set_name_np` (:64), `pthread_setaffinity_np` (:65), `pthread_getaffinity_np` (:51), `pthread_attr_get/setaffinity_np` (:48-49), `pthread_attr_get_np` (:47), `pthread_attr_setcreatesuspend_np` (:46), `pthread_getthreadid_np` (:52), `pthread_main_np` (:53), `pthread_multi_np`/`single_np` (:54/:66), suspend/resume family (:67/:63/:62/:67-68), `pthread_switch_add/delete_np` (:69-70), `pthread_timedjoin_np` (:71), `pthread_mutex_get/setspinloops_np`/`get/setyieldloops_np`/`isowned_np` (:57-61), `pthread_mutexattr_get/setkind_np` (:55-56).
> `pthread_get_name_np` is **declared** at pthread_np.h:50 but the libkernel export is `pthread_getname_np` (no underscore before `name`) — treat the declared form with care.

### Semaphores — Provider: libkernel, declared `semaphore.h:54-69`
`sem_t` at :41-47 (`struct _sem`, concrete — embeds `struct _usem2`, NOT opaque). `sem_init` (:62), `destroy` (:60), `wait` (:68), `trywait` (:66), `timedwait` (:65), `post` (:64), `getvalue` (:61), `open` (:63), `close` (:59), `unlink` (:67), `sem_clockwait_np` (:56). `SEM_FAILED`, `SEM_VALUE_MAX` at :49-50.

### Scheduling — Provider: libkernel, declared `sys/sched.h` (symlinked as `sched.h`); userland block :233-242
`sched_yield` (:241), `get_priority_max` (:234), `get_priority_min` (:235), `getparam` (:236), `setparam` (:239), `getscheduler` (:237), `setscheduler` (:240), `rr_get_interval` (:238). Policy macros `SCHED_FIFO`/`OTHER`/`RR` at :212-214; `struct sched_param` at :216-218.

### C11 threads (`threads.h`) — Provider: SDK's own `libc.a` (default-linked), declared `threads.h:76-113`
Each function is a thin wrapper over a libkernel `pthread_*`/`nanosleep` symbol. Type aliases at :43-54; `ONCE_FLAG_INIT`/`TSS_DTOR_ITERATIONS`/`thread_local` at :71-74.

| C11 function | Header line | Source / underlying call |
|---|---|---|
| `thrd_create` | threads.h:100 | `libc/thrd.c:51` → `pthread_create` |
| `thrd_current` | threads.h:101 | `libc/thrd.c:72` → `pthread_self` |
| `thrd_detach` | threads.h:102 | `libc/thrd.c:79` → `pthread_detach` |
| `thrd_equal` | threads.h:103 | `libc/thrd.c:88` → `pthread_equal` |
| `thrd_exit` | threads.h:104 | `libc/thrd.c:95` → `pthread_exit` |
| `thrd_join` | threads.h:106 | `libc/thrd.c:102` → `pthread_join` |
| `thrd_sleep` | threads.h:107 | `libc/thrd.c:114` → `nanosleep` |
| `thrd_yield` | threads.h:108 | `libc/thrd.c:121` → `pthread_yield` |
| `mtx_init`/`destroy`/`lock`/`trylock`/`timedlock`/`unlock` | threads.h:87-99 | `libc/mtx.c` → `pthread_mutex_*` (+ `pthread_mutexattr_*`) |
| `cnd_init`/`destroy`/`signal`/`broadcast`/`wait`/`timedwait` | threads.h:78-86 | `libc/cnd.c` → `pthread_cond_*` |
| `tss_create`/`delete`/`get`/`set` | threads.h:109-112 | `libc/tss.c` → `pthread_key_create`/`key_delete`/`getspecific`/`setspecific` |
| `call_once` | threads.h:77 | `libc/call_once.c:35` → `pthread_once` |

`mtx_*` map to `mtx_plain`/`mtx_recursive`/`mtx_timed` (threads.h:56-60); return codes `thrd_success`/`error`/`busy`/`nomem`/`timedout` (threads.h:62-68).

### C11 atomics (`stdatomic.h`) — Provider: NONE at link time (header-only)
`include/freebsd/stdatomic.h` (symlink to `sys/stdatomic.h`) provides `atomic_init`, `atomic_thread_fence`, `atomic_signal_fence`, `atomic_is_lock_free`, and the `atomic_{load,store,exchange,compare_exchange_strong/weak,fetch_add/and/or/sub/xor}[_explicit]` macro families plus `memory_order_*`. These are **macros over clang `__c11_atomic_*`/`__atomic_*` builtins** — no runtime symbol, not callable externs. bindgen cannot generate them; **use Rust's `core::sync::atomic` instead.**

### Symbol-only extras (exported by libkernel, NO SDK prototype) — hand-declare required
These 30 symbols are exported by `sce_stubs/libkernel*.c` (default `kernel_web`) yet have **no header declaration** under `include/`. Hand-write the `extern "C"` prototype:
`pthread_attr_get/setsolosched_np`, `pthread_barrier_setname_np`, `pthread_cond_reltimedwait_np`, `pthread_cond_setname_np`, `pthread_cond_signalto_np`, `pthread_create_name_np`, `pthread_get_specificarray_np`, `pthread_get/set/suspend/resume_user_context_np`, `pthread_getname_np`, `pthread_getstack_np`, `pthread_kill`, `pthread_mutex_init_for_mono`, `pthread_mutex_reltimedlock_np`, `pthread_mutex_setname_np`, `pthread_mutexattr_get/setgen_np`, `pthread_rename_np`, `pthread_rwlock_reltimedrdlock_np`, `pthread_rwlock_reltimedwrlock_np`, `pthread_rwlock_setname_np`, `pthread_rwlockattr_get/settype_np`, `pthread_set_defaultstacksize_np`, `pthread_sigmask`, `sem_reltimedwait_np`, `sem_setname`. (`pthread_getname_np` is the real export backing the header's misspelled `pthread_get_name_np`.)

**L2 net:** `pthread_*`/`sem_*`/`sched_*` are declared and resolve from default `-lkernel_web` — directly bindgen-able, link-clean. C11 `thrd_*`/`mtx_*`/`cnd_*`/`tss_*`/`call_once` are declared and resolve from the default `libc.a`. `stdatomic.h` is builtin-only — use Rust native atomics. The 30 `_np`/Sony-specific exports are symbol-only.

Files: `include/freebsd/{pthread.h,pthread_np.h,semaphore.h,threads.h}`, `include/freebsd/sys/{sched.h,stdatomic.h,_pthreadtypes.h}`, `libc/{thrd,mtx,cnd,tss,call_once}.c`, `libc/Makefile`, `sce_stubs/libkernel_web.c`.

---

## 6. Sony SCE library catalog (symbol-only)

Every stub library lives under `sce_stubs/*.c` — assembler stub lists declaring symbols with `.global NAME` and **no C prototype in the SDK** for these `sce*`/`mono_*`/`coil_*`/`egl*` entries (unless a matching `include/` header exists, which for these does not). Therefore every API below is **symbol-only**: hand-write the `extern "C"` prototype to call it from Rust. Default-linked libs: `{libc (SDK's own), SceLibcInternal, SceNet, kernel_web}`; all others need an explicit `-lSce…` / `-lkernel…`.

Symbol counts = unique `.global` per file (`grep -oE '\.global [A-Za-z0-9_]+' <lib>.c | awk '{print $2}' | sort -u | wc -l`).

| Library | Purpose | Symbols | Default-linked? | Examples |
|---|---|---|---|---|
| `libkernel.c` | Base kernel/libc runtime: POSIX/BSD syscalls, pthreads, sockets, mmap | 1137 | No (`-lkernel`) | `pthread_create`, `socket`, `connect`, `open`, `read`, `mmap`, `nanosleep`, `__error` |
| `libkernel_web.c` | Default kernel variant used by prospero-clang; same surface | 1111 | **Yes** (`-lkernel_web`) | `pthread_create`, `socket`, `read`, `write`, `mmap`, `__inet_pton`, `__pthread_cleanup_push_imp` |
| `libkernel_sys.c` | System kernel variant (largest); adds internal syscalls like `__getcwd` | 1281 | No (`-lkernel_sys`) | `__getcwd`, `pthread_create`, `socket`, `open`, `mmap`, `_fstat`, `__sys_osem_open` |
| `libSceLibcInternal.c` | Sony's internal C stdlib: stdio, malloc, string, math, atomics | 3015 | **Yes** (`-lSceLibcInternal`) | `malloc`, `free`, `calloc`, `printf`, `snprintf`, `memcpy`, `memset`, `strlen`, `qsort`, `fopen` |
| `libSceNet.c` | Berkeley-style socket + IPv4/IPv6 networking (Sony `sceNet*`) | 220 | **Yes** (`-lSceNet`) | `sceNetSocket`, `sceNetAccept`, `sceNetBind`, `sceNetClearDnsCache`, `in6addr_any` |
| `libSceNetCtl.c` | Network control / connection state, BWE, AP config | 95 | No (`-lSceNetCtl`) | `sceNetCtlApAppStartWithRetry`, `sceNetBweGetInfoIpcInt`, `sceNetCtlApAppInitWpaKey` |
| `libSceSsl.c` | TLS/SSL plus certificate/key crypto helpers | 54 | No (`-lSceSsl`) | `sceSslConnect`, `sceSslCreateConnection`, `RSA_verifySignature`, `CA_MGMT_extractPublicKeyInfo` |
| `libSceHttp.c` | HTTP/1.x client: requests, headers, cookies, auth cache | 115 | No (`-lSceHttp`) | `sceHttpAddRequestHeader`, `sceHttpAddCookie`, `sceHttpAddQuery`, `sceHttpAuthCacheExport` |
| `libSceHttp2.c` | HTTP/2 client (URL-based request/template/cookie-box API) | 56 | No (`-lSceHttp2`) | `sceHttp2CreateRequestWithURL`, `sceHttp2AddRequestHeader`, `sceHttp2CreateTemplate`, `sceHttp2CreateCookieBox` |
| `libSceSystemService.c` | Shell/system-service control: app messaging, LNC util, CPU budgets | 517 | No (`-lSceSystemService`) | `sceAppMessagingSendMsg`, `sceAppMessagingReceiveMsg`, `sceLncUtilGetAppStatus`, `sceLncUtilGetCoredumpState` |
| `libSceUserService.c` | User/account info, settings, per-user status | 484 | No (`-lSceUserService`) | `sceUserServiceGetLoginUserIdList`, `sceUserServiceGetUserStatus`, `sceUserServiceGetAgeLevel` |
| `libSceFsInternalForVsh.c` | VSH-internal filesystem / Blu-ray disc scheduler | 244 | No (`-lSceFsInternalForVsh`) | `sceBdSchedConfigure`, `sceBdSchedGetBackgroundCopyRequest`, `sceBdSchedGetExtentMap`, `sceBdSchedGetState` |
| `libSceGLSlimVSH.c` | EGL/OpenGL ES "slim" graphics binding for VSH | 203 | No (`-lSceGLSlimVSH`) | `eglChooseConfig`, `eglCreateContext`, `eglCreateWindowSurface`, `eglBindAPI`, `eglGetConfigs` |
| `libSceGnmDriver.c` | GNM low-level GPU command-submission driver | 163 | No (`-lSceGnmDriver`) | `sceGnmCreateWorkloadStream`, `sceGnmAddEqEvent`, `sceGnmBeginWorkload`, `sceGnmDingDong` |
| `libSceGnmDriverForNeoMode.c` | GNM GPU driver, PS4-Pro/"Neo" mode variant (mirrors `libSceGnmDriver`) | 163 | No (`-lSceGnmDriverForNeoMode`) | `sceGnmCreateWorkloadStream`, `sceGnmAddEqEvent`, `sceGnmComputeWaitOnAddress` |
| `libSceVideoOut.c` | Video output / display: framebuffer registration, flip & vblank events | 169 | No (`-lSceVideoOut`) | `sceVideoOutAddBuffer`, `sceVideoOutAddFlipEvent`, `sceVideoOutAddOutputModeEvent` |
| `libSceAudioOut.c` | Audio output (AudioOut/AudioOut2) and device control | 136 | No (`-lSceAudioOut`) | `sceAudioOut2ContextCreate`, `sceAudioOut2ContextPush`, `sceAudioDeviceControlGet/Set` |
| `libScePad.c` | DualSense/DualShock gamepad input and device management | 130 | No (`-lScePad`) | `scePadOpen`/`scePadClose`, `scePadConnectPort`, `scePadDeviceClassParseData` |
| `libSceAppInstUtil.c` | App install/uninstall, move, size/metadata utilities | 108 | No (`-lSceAppInstUtil`) | `sceAppInstUtilAppExists`, `sceAppInstUtilAppUnInstall`, `sceAppInstUtilAppGetAddcontInfo` |
| `libSceIpmi.c` | IPMI client IPC (C++ `IPMI::impl::ClientImpl` — mangled symbols) | 95 | No (`-lSceIpmi`) | `_ZN4IPMI4impl10ClientImpl10initializeE…`, `_ZN4IPMI4impl10ClientImpl16invokeSyncMethodE…` |
| `libSceSysCore.c` | SysCore process management: spawn/kill/continue/crash of system processes | 78 | No (`-lSceSysCore`) | `sceApplicationAddProcess2`, `sceApplicationExitSpawn`, `sceApplicationBlockingKill2` |
| `libSceRegMgr.c` | System registry manager (get/set values, backup) | 64 | No (`-lSceRegMgr`) | `sceRegMgrCheckIntVal`, `sceRegMgrBackupPushData`, `sceRegMgrCntlDeleteReg` |
| `libSceRemoteplay.c` | Remote Play: connection, registration, pin code, connect history | 43 | No (`-lSceRemoteplay`) | `sceRemoteplayApprove`, `sceRemoteplayGeneratePinCode`, `sceRemoteplayGetConnectionStatus` |
| `libScePosixForWebKit.c` | POSIX shims for WebKit (`__wrap_*` fs/env wrappers + arc4random) | 36 | No (`-lScePosixForWebKit`) | `__wrap_mmap`, `__wrap_access`, `__wrap_getcwd`, `__wrap_getenv`, `__wrap_statvfs`, `arc4random` |
| `libSceKeyboard.c` | Keyboard input (incl. debug `sceDbgKeyboard*`) | 22 | No (`-lSceKeyboard`) | `sceKeyboardOpen`/`Close`, `sceKeyboardConnectPort`, `sceKeyboardRead`, `sceDbgKeyboardInit` |
| `libSceSysmodule.c` | System module loader (load/unload/query PRX modules) | 16 | No (`-lSceSysmodule`) | `sceSysmoduleLoadModule`, `sceSysmoduleIsLoaded`, `sceSysmoduleMapLibcForLibkernel` |
| `libSceImeDialog.c` | On-screen IME (text input) dialog | 14 | No (`-lSceImeDialog`) | `sceImeDialogInit`, `sceImeDialogGetStatus`, `sceImeDialogGetResult`, `sceImeDialogAbort` |
| `libSceNotification.c` | System pop-up notifications | 2 | No (`-lSceNotification`) | `sceNotificationSend`, `sceNotificationSendById` |
| `libSceRandom.c` | Secure random number source | 1 | No (`-lSceRandom`) | `sceRandomGetRandomNumber` |
| `libmonosgen-2.0.c` | Mono runtime (SGen GC) embedding API + Sony `coil_*` OS shims | 931 | No (`-lmonosgen-2.0`) | `mono_assembly_open`, `mono_class_describe_statics`, `coil_dlopen_native`, `coil_socket` |

Notes:
- `libSceGnmDriver.c` and `libSceGnmDriverForNeoMode.c` export the same 163-symbol `sceGnm*` set (Neo/Pro hardware-mode variant).
- All three kernel stubs overlap heavily on the POSIX/BSD/pthread surface; `kernel_web` is the prospero-clang default, so its `socket`/`open`/`read`/`pthread_*`/`mmap` symbols resolve without a flag.
- Names sampled from family membership (e.g. `scePadOpen`, `sceKeyboardRead`, `sceAppInstUtilAppUnInstall`) are representative; the other listed examples are verbatim `.global` names.
- These are stub source files; the `ps5/*` kernel APIs come from `crt1.o` (§1) and are not in this catalog.

---

## 7. Other SDK-provided surfaces (libc.a extras, libufs, Khronos, crt)

The default include search adds one root — `${PS5_PAYLOAD_SDK}/target/include` (`prospero-clang:74`, `-isystem`) — into which `include/Makefile` copies the FreeBSD headers flat, the Khronos headers flat, and the PS5 headers under `ps5/`. So every entry below is reachable through one include root.

### SDK `libc.a` extras (Provider: `libc.a`, default-linked)

`libc/Makefile` builds `libc.a` from `$(wildcard *.c)` plus `_setjmp.o`; it also emits empty `libdl.a` and `libpthread.a` archives (real symbols live in `libc.a` / `kernel_web`, so `-ldl`/`-lpthread` are no-ops kept for build-system compatibility). All of the following are **declared** (FreeBSD prototypes exist in `include/freebsd/**`), so bindgen generates them directly — gap-fill implementations the SDK provides because Sony's libc does not export them.

- **dlfcn** (`libc/dlfcn.c`): `dlopen`, `dlsym`, `dlclose`, `dladdr`, `dlerror` — thin wrappers over the runtime linker's weak `__dlopen/__dlsym/__dlclose/__dladdr/__dlerror` (`crt/rtld_payload.c`). Header `include/freebsd/dlfcn.h`.
- **backtrace** (`libc/backtrace.c`): `backtrace`, `backtrace_symbols`, `backtrace_symbols_fd` (`include/freebsd/execinfo.h`).
- **arc4random** (`libc/arc4random.c`): `arc4random`, `arc4random_buf`, `arc4random_uniform`, `arc4random_stir`, `arc4random_addrandom`.
- **mman wrappers** (`libc/mman.c`): `mmap`, `mprotect` — routed through `sys_mmap`/`sys_mprotect` and a weak `kernel_mprotect` (`libc/mman.c:27`); `posix_madvise` (`libc/pmadvise.c`).
- **mount syscalls** (`libc/mount.c`): `mount`, `nmount`, `unmount` (direct `__syscall6`); `getmntinfo` (`libc/getmntinfo.c`).
- **filesystem traversal**: `fts_open/read/children/set/close` (`libc/fts.c`), `nftw` (`libc/nftw.c`), `scandir`/`scandir_b`/`alphasort` (`libc/scandir.c`), `dirfd` (`libc/dirfd.c`).
- **pattern matching / regex**: `fnmatch` (`libc/fnmatch.c`); full POSIX regex `regcomp`/`regexec`/`regerror`/`regfree` (`libc/regcomp.c`, `regexec.c`, `regerror.c`, `regfree.c`). No `glob.c` — `glob` is not gap-filled.
- **temp files**: `mkstemp`, `mkostemp`, `mkstemps`, `mkostemps`, `mkdtemp`, `mktemp`, `_mktemp` (`libc/mktemp.c`); `tmpfile` (`libc/tmpfile.c`), `tmpnam` (`libc/tmpnam.c`).
- **C11 threads** (declared, gap-filled on pthreads): `thrd_*` / `mtx_*` / `cnd_*` / `tss_*` / `call_once` (`libc/{thrd,mtx,cnd,tss,call_once}.c`) — see §5.
- **networking name resolution** (`libc/netdb.c`): `getaddrinfo`, `freeaddrinfo`, `getnameinfo`, `gethostbyname`/`_r`, `gethostbyaddr`, `getservbyname`/`_r`, `getservbyport`/`_r`, `getprotobyname`, `gai_strerror`; `if_indextoname`/`if_nametoindex` (`libc/if_*.c`), `recvmmsg`/`sendmmsg` (`libc/recvmmsg.c`, `sendmmsg.c`), `pipe2` (`libc/pipe2.c`).
- **search trees**: `tsearch`/`tfind`/`tdelete`/`twalk` (`libc/tsearch.c`, `tfind.c`, `tdelete.c`, `twalk.c`); `qsort_r` (`libc/qsort_r.c`).
- **pwd/grp**: `getpwnam`/`getpwuid`(`_r`) (`libc/passwd.c`), `getgrnam`/`getgrgid` (`libc/grp.c`).
- **time**: `localtime_r` (`libc/localtime.c`), `gmtime`/`gmtime_r` (`libc/gmtime.c`), `asctime_r`, `ctime_r`, `times` (`libc/times.c`).
- **misc**: `popen`/`pclose` (`libc/popen.c`), `execlp` (`libc/execlp.c`), `syslog`/`openlog`/`closelog` (`libc/syslog.c`), `strsignal` (`libc/strsignal.c`), `strcasecmp`/`strcasestr`, `reallocarray` (`libc/reallocarray.c`), `memccpy`, `ffs/ffsl/ffsll`/`fls/flsl/flsll`, `getloadavg` (`libc/getloadavg.c`), termios `tcgetattr`/`tcsetattr`/`cfmakeraw`/… (`libc/termios.c`), `isatty` (`libc/isatty.c`), locale stubs (`libc/no-locale.c`, `nl_langinfo.c`).
- **raw-syscall surface** (`libc/syscalls.c`): hundreds of FreeBSD syscall thunks via the SDK's `__syscall` macro — e.g. `kldload`/`kldunload`/`kldstat`, `jail`/`jail_set`/`jail_get`, `cap_enter`/`cap_rights_limit`, `extattr_*`, `__acl_*`, `__mac_*`, `statfs`/`fstatfs`/`getfsstat`, `procctl`, `ptrace`, `thr_new`/`thr_create`/`thr_self`, `posix_fadvise`/`posix_fallocate`, `openat`/`fstatat`/`unlinkat`/`renameat`, `chroot`, `reboot`, etc. Headers: `include/freebsd/sys/syscall.h` plus per-API FreeBSD headers.

### libufs (Provider: `libufs.a` — opt-in, link with `-lufs`)

`libufs/Makefile` builds `libufs.a` from its `*.c` (compiled with `-D_LIBUFS`); **not** default-linked. All functions are **declared** in `libufs/libufs.h` (FreeBSD-style UFS userland disk access over a `struct uufsd`):

- Disk open/close: `ufs_disk_fillout`, `ufs_disk_fillout_blank`, `ufs_disk_close`, `ufs_disk_write` (`libufs/type.c`).
- Superblock: `sbread`, `sbwrite` (`libufs/sblock.c`).
- Cylinder groups: `cgread`, `cgwrite`, `cgballoc`, `cgbfree`, `cgialloc` (`libufs/cgroup.c`).
- Inodes: `getino`, `putino` (`libufs/inode.c`).
- Raw block I/O: `bread`, `bwrite`, `berase` (`libufs/block.c`).

Purpose: read/modify a UFS filesystem image at block/superblock/cgroup/inode level from a payload.

### Khronos graphics headers (Provider: headers only; needs Sony GPU stubs — opt-in)

At `include/khronos/`, copied flat into `target/include`, so `#include <EGL/egl.h>` / `#include <GLES2/gl2.h>` work by default.

- **EGL** (`include/khronos/EGL/{egl.h,eglext.h,eglplatform.h}`): full EGL native-platform API **declared**, `EGL_VERSION_1_0`–`1_5` (per the `EGL_VERSION_1_x` guards), plus extensions in `eglext.h`.
- **OpenGL ES 2.0** (`include/khronos/GLES2/{gl2.h,gl2ext.h,gl2platform.h}`): `GL_ES_VERSION_2_0` API **declared**, plus extensions in `gl2ext.h`.
- **KHR** (`include/khronos/KHR/khrplatform.h`): shared Khronos platform typedefs.

> These are only header declarations — there is no `libGLESv2`/`libEGL` in the SDK. The actual `egl*`/`gl*` symbols come from opt-in Sony GPU stubs in `sce_stubs/`: `libSceVideoOut.c`, `libSceGnmDriver.c` (+ `libSceGnmDriverForNeoMode.c`), `libSceGLSlimVSH.c` (plus `libSceFsInternalForVsh.c`). Expect to add `-lSceVideoOut -lSceGnmDriver -lSceGLSlimVSH`. Individual `gl*`/`egl*` entry points are not enumerated here.

### C runtime objects (Provider: crt; `crt1.o` default-linked)

`crt/Makefile` produces seven runtime objects installed into `target/lib`:

- **`crt1.o`** — the real startup object, relocatably linked (`ld -m elf_x86_64 -r`) from `crt.o syscall.o klog.o nid.o kernel.o rtld.o rtld_so.o rtld_sprx.o rtld_payload.o rtld_dlfcn.o mdbg.o patch.o`. Provides `_start` (`crt/crt.c:197`), entered with a `payload_args_t*` (`crt/payload.h`); zeroes BSS, runs `payload_init` → `__crt_syscall_init`/`__kernel_init`/`__klog_init`/`__patch_init`/`__rtld_init`, and exports `payload_get_args`/`payload_exit`. **This object links in the entire `ps5/*` API** (§1).
- **`crti.o`, `crtn.o`, `crtbegin.o`, `crtend.o`, `crtbeginS.o`, `crtendS.o`** — all created as **empty archives** (`$(AR) -rsc $@` with no inputs), so the standard GCC/Clang crt link-line slots resolve to no-ops on this platform.
