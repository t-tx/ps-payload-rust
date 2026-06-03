/* Feature `thread`: POSIX threads + C11 threads + semaphores + scheduling
 * (all declared). pthread, sem and sched symbols come from kernel_web; the C11
 * thrd/mtx/cnd/tss/call_once layer from the SDK libc.a (both default-linked). */
#include <pthread.h>      /* pthread_create/join/mutex/cond/rwlock/key/...  */
#include <pthread_np.h>   /* pthread_set_name_np, setaffinity_np, ...       */
#include <semaphore.h>    /* sem_init/wait/post/destroy, sem_t              */
#include <sched.h>        /* sched_yield, sched_get_priority_*              */
#include <threads.h>      /* C11: thrd_*, mtx_*, cnd_*, tss_*, call_once    */
