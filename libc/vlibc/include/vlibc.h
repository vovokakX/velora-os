/*
 * vlibc C API (build vlibc with the "cabi" feature).
 * Only a small subset exists so far.
 *
 * SPDX-License-Identifier: LGPL-3.0-or-later
 */
#ifndef VLIBC_H
#define VLIBC_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void exit(int code) __attribute__((noreturn));
void _exit(int code) __attribute__((noreturn));
long getpid(void);

/* fd 0 = keyboard, 1 = stdout, 2 = stderr. Return -1 on error. */
long write(int fd, const void *buf, size_t len);
long read(int fd, void *buf, size_t len);
int puts(const char *s);

/* Files. Paths are absolute. Functions return -1 on error. */
#define O_RDONLY    0
#define O_WRONLY    1
#define O_RDWR      2
#define O_CREAT     0100
#define O_EXCL      0200
#define O_TRUNC     01000
#define O_APPEND    02000
#define O_DIRECTORY 0200000
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

int open(const char *path, int flags, int mode);
int close(int fd);
long lseek(int fd, long offset, int whence);
int mkdir(const char *path, int mode);
int unlink(const char *path);
int rename(const char *from, const char *to);
void sync(void);

void *malloc(size_t size);
void *calloc(size_t n, size_t size);
void *realloc(void *p, size_t size);
void free(void *p);

#ifdef __cplusplus
}
#endif

#endif /* VLIBC_H */
