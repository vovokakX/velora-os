/*
 * vlibc C API (build vlibc with the "cabi" feature).
 * Only a small subset exists so far; file functions arrive with the
 * file systems (stage 7).
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

void *malloc(size_t size);
void *calloc(size_t n, size_t size);
void *realloc(void *p, size_t size);
void free(void *p);

#ifdef __cplusplus
}
#endif

#endif /* VLIBC_H */
