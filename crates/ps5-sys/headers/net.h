/* Feature `net`: BSD socket stack (declared; socket syscalls from kernel_web,
 * inet and getaddrinfo from SceLibcInternal -- both default-linked). */
#include <sys/socket.h>   /* socket, bind, connect, send, recv, setsockopt  */
#include <netinet/in.h>   /* sockaddr_in, in_addr, IPPROTO, htons macros    */
#include <netinet/tcp.h>  /* TCP_NODELAY                                    */
#include <arpa/inet.h>    /* inet_pton, inet_ntop, inet_addr, inet_aton     */
#include <netdb.h>        /* getaddrinfo, getnameinfo, addrinfo, hostent    */
#include <net/if.h>       /* if_nametoindex, if_indextoname                 */
#include <poll.h>         /* poll, struct pollfd, POLL flags                */
#include <sys/select.h>   /* select, fd_set, FD macros                      */
#include <sys/ioctl.h>    /* ioctl, FIONBIO                                 */
#include <fcntl.h>        /* fcntl + O_NONBLOCK (non-blocking sockets)      */
#include <unistd.h>       /* close (sockets are fds)                        */
