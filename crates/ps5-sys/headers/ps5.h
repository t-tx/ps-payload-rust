/* Core translation unit handed to bindgen (always generated).
 *
 * Pulls in the PS5-specific public headers of the ps5-payload-sdk, plus the
 * minimal errno/strerror surface every higher layer needs for error reporting.
 * The ps5 symbols are provided at link time by crt1.o; errno helpers come
 * from kernel_web and SceLibcInternal (both default-linked). See CLAUDE.md.
 *
 * The libc (FreeBSD) file/socket/thread surfaces live in the per-feature
 * headers (fs.h, net.h, thread.h); HTTP/TLS is hand-written (src/sce.rs).
 */
#include <ps5/payload.h>
#include <ps5/kernel.h>
#include <ps5/klog.h>
#include <ps5/mdbg.h>
#include <ps5/nid.h>

/* errno location + numeric codes + strerror, for L2 error mapping */
#include <errno.h>
#include <string.h>
