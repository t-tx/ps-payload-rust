/* Feature `fs`: POSIX/BSD file & filesystem surface (all declared in FreeBSD
 * headers; symbols from kernel_web + SceLibcInternal, both default-linked). */
#include <fcntl.h>      /* open, openat, creat, O_* flags                  */
#include <unistd.h>     /* read, write, close, lseek, dup, unlink, ...     */
#include <sys/stat.h>   /* stat, fstat, lstat, mkdir, chmod, struct stat   */
#include <sys/types.h>
#include <dirent.h>     /* opendir, readdir, closedir, DIR, struct dirent  */
#include <sys/uio.h>    /* readv, writev, struct iovec                     */
#include <sys/mman.h>   /* mmap, munmap, mprotect, msync, PROT and MAP    */
#include <stdio.h>      /* fopen, fread, fwrite, fclose, FILE, rename, ... */
