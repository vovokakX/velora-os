# vlibc

Библиотека для программ Velora (`libc/vlibc`), с этапа 5.

- `_start`, разбор argc/argv/envp, макрос `vlibc::main!`
- системные вызовы (`sys`), вывод `print!`/`println!`/`eprintln!`, ввод с клавиатуры
- куча: работает `alloc` (`Vec`, `String`, `BTreeMap`); тот же аллокатор, что
  и в ядре (`velora-mm`), растёт через `mmap`
- процессы: `spawn`, `wait`, `exit`, `getpid`, `yield_now`; время: `monotonic`, `sleep_ms`
- `mmap`/`munmap`
- C API (feature `cabi`, заголовок `include/vlibc.h`): `exit`, `write`, `read`,
  `puts`, `getpid`, `malloc`, `calloc`, `realloc`, `free`

Лицензия: LGPL-3.0-or-later (`libc/vlibc/LICENSE`; LGPL дополняет GPL-3.0 из
`LICENSE` в корне). Программы с любой лицензией могут компоноваться с vlibc.
Код из `kernel/abi` и `kernel/mm`, который входит в vlibc, тоже под LGPL.

Ограничения: программы однопоточные (куча без блокировок). Файлов пока нет,
они появятся на этапе 7.

---

The library for Velora programs (`libc/vlibc`), since stage 5: `_start` and
argc/argv/envp parsing with the `vlibc::main!` macro; system calls, output
macros and keyboard input; a heap so `alloc` works (the kernel's allocator from
`velora-mm`, growing with `mmap`); processes (`spawn`, `wait`, `exit`, `getpid`,
`yield_now`), time (`monotonic`, `sleep_ms`), `mmap`/`munmap`; a C API behind the
`cabi` feature (`include/vlibc.h`). License: LGPL-3.0-or-later (`libc/vlibc/LICENSE`; the LGPL supplements the
GPL-3.0 in the root `LICENSE`), so programs under any license can link with
vlibc. The code from `kernel/abi` and `kernel/mm` that vlibc includes is LGPL
too. Limitations: programs are single-threaded
(the heap has no locking); files arrive in stage 7.
