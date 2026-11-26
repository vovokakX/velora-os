# Системные вызовы / System calls

[Русский](#русский) | [English](#english)

## Русский

Состояние: работают вызовы из таблицы «Реализовано» (этапы 4–5). Номера и коды
ошибок определены в crate `velora-abi` (`kernel/abi`): им пользуются и ядро,
и будущая vlibc.

### Соглашение о вызове (нативный Velora ABI)

- Инструкция `syscall`. Номер вызова в `rax`.
- Аргументы: `rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9` (`r10` вместо `rcx`,
  потому что `syscall` записывает в `rcx` адрес возврата).
- Результат в `rax`: значение `>= 0` — успех, отрицательное — код ошибки
  (`-errno`).
- `rcx` и `r11` портятся инструкцией `syscall`. Остальные регистры
  сохраняются.
- Все указатели проверяются по таблицам страниц процесса: память должна быть
  отображена для пользователя, а для записи ещё и доступна на запись.
  Иначе возвращается `-EFAULT`.
- Внутри системного вызова прерывания выключены, и ядро не вытесняется.
  Блокирующие вызовы (`read`, `sleep_ms`, `yield`) явно передают процессор
  другому потоку.

### Реализовано

| № | Вызов | Описание | Ошибки |
|---|---|---|---|
| 0 | `exit(status)` | завершает процесс со статусом (младшие 32 бита со знаком) | — |
| 1 | `write(fd, buf, len)` | fd 1 и 2 — консоль; не больше 64 КиБ за вызов; неверный UTF-8 выводится как U+FFFD; возвращает число байт | `EBADF`, `EFAULT` |
| 2 | `read(fd, buf, len)` | fd 0 — клавиатура; ждёт хотя бы один символ, возвращает целые символы UTF-8 (`len >= 4`) | `EBADF`, `EINVAL`, `EFAULT` |
| 3 | `yield()` | отдаёт процессор другим готовым потокам | — |
| 4 | `getpid()` | идентификатор процесса | — |
| 5 | `spawn(path, path_len, argv, argc, envp, envc)` | запускает программу из файла; `argv`/`envp` — массивы `StrRef { ptr, len }`; возвращает pid потомка | `ENOENT`, `ENOEXEC`, `E2BIG`, `ENOMEM`, `EFAULT`, `EINVAL` |
| 6 | `wait(pid, status)` | ждёт завершения потомка, пишет его статус (`i64`) по адресу `status` (может быть 0); возвращает pid | `ECHILD`, `EFAULT` |
| 7 | `mmap(addr, len, prot, flags)` | анонимная память, заполненная нулями; `prot`: READ 1, WRITE 2, EXEC 4 (WRITE+EXEC запрещено); `flags`: FIXED 1 — ровно по `addr` | `EINVAL`, `EEXIST`, `ENOMEM` |
| 8 | `munmap(addr, len)` | убирает отображение и освобождает страницы | `EINVAL` |
| 21 | `clock_gettime(clock, ts)` | `clock = 1` (MONOTONIC): время с загрузки; `clock = 0` (REALTIME, с этапа 6): время UTC с 1970 года по часам RTC; `struct { u64 sec; u64 nsec; }` | `EINVAL`, `EAGAIN` (время неизвестно), `EFAULT` |
| 25 | `sleep_ms(ms)` | спит не меньше `ms` миллисекунд (точность — тик 10 мс) | — |

Неизвестный номер возвращает `-ENOSYS` (38).

### Коды ошибок

`EPERM 1, ENOENT 2, ESRCH 3, EINTR 4, EIO 5, E2BIG 7, ENOEXEC 8, EBADF 9,
ECHILD 10, EAGAIN 11, ENOMEM 12, EFAULT 14, EEXIST 17, EINVAL 22, ENOSYS 38` (значения как в Linux, для удобства
переноса программ).

### Ошибки процессора в программах

Если программа вызывает исключение процессора (page fault, #GP, #UD и т. д.),
ядро выводит строку с причиной и завершает процесс со статусом
`128 + номер исключения` (page fault → 142, #GP → 141). Ядро продолжает работу.

### Селекторы (из `kernel/src/arch/x86_64/gdt.rs`)

| Селектор | Значение |
|---|---|
| 0x08 | код ядра |
| 0x10 | данные ядра |
| 0x18 | код пользователя, 32 бита (нужен только для раскладки `sysret`) |
| 0x23 | данные пользователя |
| 0x2b | код пользователя, 64 бита |
| 0x30 | TSS |

`STAR[47:32] = 0x08`, `STAR[63:48] = 0x18`. `FMASK` сбрасывает IF, DF, TF и AC.

### Personality

Номер вызова ищется в таблице той personality, к которой относится процесс
(`sched::Personality`). Сейчас есть только `Native`. Программы Win32 (в
будущем) не будут вызывать ядро напрямую: это будет делать Win32-слой в
пространстве пользователя через нативную таблицу.

### Запланировано

| № | Вызов | Этап |
|---|---|---|
| 9 | `open(path, flags, mode)` | 7 |
| 10 | `close(fd)` | 7 |
| 11 | `seek(fd, off, whence)` | 7 |
| 12 | `stat(path, buf)` | 7 |
| 13 | `readdir(fd, buf, len)` | 7 |
| 14 | `mkdir(path, mode)` | 7 |
| 15 | `unlink(path)` | 7 |
| 16 | `rename(old, new)` | 7 |
| 17 | `pipe(fds)` | 8 |
| 18 | `dup2(old, new)` | 8 |
| 19 | `chdir(path)` | 8 |
| 20 | `kill(pid, sig)` | 8 |
| 22 | `sysinfo(buf)` | 8 |
| 23 | `reboot(cmd)` | 8 |
| 24 | `setuid(uid)` / `getuid()` | 8 |

Изменения фиксируются здесь и в [decisions.md](decisions.md).

## English

Status: since stage 4 the calls in the "Implemented" table work. Numbers and
error codes are defined in the `velora-abi` crate (`kernel/abi`), shared by
the kernel and the future vlibc.

### Calling convention (native Velora ABI)

- `syscall` instruction, call number in `rax`.
- Arguments in `rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9` (`r10` replaces `rcx`
  because `syscall` stores the return address in `rcx`).
- Result in `rax`: `>= 0` on success, negative error code (`-errno`) on failure.
- `rcx` and `r11` are clobbered by `syscall`; all other registers are preserved.
- Every pointer is checked against the process page tables: the memory must be
  mapped for user access (and writable when written to), otherwise the call
  returns `-EFAULT`.
- Interrupts are off inside system calls and the kernel is not preempted there.
  Blocking calls (`read`, `sleep_ms`, `yield`) give up the CPU explicitly.

### Implemented

| No. | Call | Description | Errors |
|---|---|---|---|
| 0 | `exit(status)` | ends the process with the status (low 32 bits, signed) | — |
| 1 | `write(fd, buf, len)` | fd 1 and 2 are the console; at most 64 KiB per call; invalid UTF-8 is shown as U+FFFD; returns the byte count | `EBADF`, `EFAULT` |
| 2 | `read(fd, buf, len)` | fd 0 is the keyboard; waits for at least one character and returns whole UTF-8 characters (`len >= 4`) | `EBADF`, `EINVAL`, `EFAULT` |
| 3 | `yield()` | gives the CPU to other ready threads | — |
| 4 | `getpid()` | process id | — |
| 5 | `spawn(path, path_len, argv, argc, envp, envc)` | starts a program from a file; `argv`/`envp` are arrays of `StrRef { ptr, len }`; returns the child pid | `ENOENT`, `ENOEXEC`, `E2BIG`, `ENOMEM`, `EFAULT`, `EINVAL` |
| 6 | `wait(pid, status)` | waits for a child to exit and stores its status (`i64`) at `status` (may be 0); returns the pid | `ECHILD`, `EFAULT` |
| 7 | `mmap(addr, len, prot, flags)` | zero-filled anonymous memory; `prot`: READ 1, WRITE 2, EXEC 4 (WRITE+EXEC refused); `flags`: FIXED 1 — exactly at `addr` | `EINVAL`, `EEXIST`, `ENOMEM` |
| 8 | `munmap(addr, len)` | removes mappings and frees the pages | `EINVAL` |
| 21 | `clock_gettime(clock, ts)` | `clock = 1` (MONOTONIC): time since boot; `clock = 0` (REALTIME, since stage 6): UTC time since 1970 from the RTC; `struct { u64 sec; u64 nsec; }` | `EINVAL`, `EAGAIN` (time unknown), `EFAULT` |
| 25 | `sleep_ms(ms)` | sleeps at least `ms` milliseconds (10 ms tick resolution) | — |

An unknown number returns `-ENOSYS` (38). Error numbers match Linux to make
porting easier. A program that raises a CPU exception is reported and ends
with status `128 + vector` (page fault → 142, #GP → 141); the kernel keeps
running. Selectors, STAR/FMASK, personality and the planned calls are listed
in the Russian section above.
