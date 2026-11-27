# Системные вызовы / System calls

[Русский](#русский) | [English](#english)

## Русский

Состояние: работают вызовы из таблицы «Реализовано» (этапы 4–10). Номера и коды
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
| 1 | `write(fd, buf, len)` | файл: пишет с текущей позиции (с `APPEND` — в конец), не больше 1 МиБ за вызов; консоль: не больше 64 КиБ, неверный UTF-8 выводится как U+FFFD; возвращает число байт | `EBADF`, `EFAULT`, `ENOSPC`, `EFBIG`, `EROFS`, `EIO` |
| 2 | `read(fd, buf, len)` | файл: читает с текущей позиции, не больше 1 МиБ, 0 — конец файла; консоль: ждёт хотя бы один символ, возвращает целые символы UTF-8 (`len >= 4`) | `EBADF`, `EISDIR`, `EINVAL`, `EFAULT`, `EIO` |
| 3 | `yield()` | отдаёт процессор другим готовым потокам | — |
| 4 | `getpid()` | идентификатор процесса | — |
| 5 | `spawn(path, path_len, argv, argc, envp, envc)` | запускает программу из файла; `argv`/`envp` — массивы `StrRef { ptr, len }`; возвращает pid потомка | `ENOENT`, `ENOEXEC`, `E2BIG`, `ENOMEM`, `EFAULT`, `EINVAL` |
| 6 | `wait(pid, status)` | ждёт завершения потомка, пишет его статус (`i64`) по адресу `status` (может быть 0); возвращает pid | `ECHILD`, `EFAULT` |
| 7 | `mmap(addr, len, prot, flags)` | анонимная память, заполненная нулями; `prot`: READ 1, WRITE 2, EXEC 4 (WRITE+EXEC запрещено); `flags`: FIXED 1 — ровно по `addr` | `EINVAL`, `EEXIST`, `ENOMEM` |
| 8 | `munmap(addr, len)` | убирает отображение и освобождает страницы | `EINVAL` |
| 9 | `open(path, path_len, flags, mode)` | открывает файл, возвращает наименьший свободный дескриптор (не больше 64 на процесс); флаги — ниже; `mode` — права нового файла | `ENOENT`, `EEXIST`, `EACCES`, `EISDIR`, `ENOTDIR`, `EROFS`, `EMFILE`, `ENAMETOOLONG`, `EINVAL`, `EFAULT` |
| 10 | `close(fd)` | закрывает дескриптор | `EBADF` |
| 11 | `seek(fd, offset, whence)` | `whence`: SET 0, CURRENT 1, END 2; возвращает новую позицию | `EBADF`, `EINVAL`, `ESPIPE` (консоль) |
| 12 | `stat(path, path_len, out)` | пишет `Stat` (64 байта): dev, ino, mode (тип и права), nlink, uid, gid, size, allocated, mtime, ctime | `ENOENT`, `ENOTDIR`, `EACCES`, `EFAULT` |
| 13 | `readdir(fd, buf, len)` | записи `DirentHeader` (ino, rec_len, name_len, kind) + имя, выровненные по 8 байт; 0 — конец каталога; позиция хранится в дескрипторе | `ENOTDIR`, `EINVAL` (запись не помещается), `EFAULT` |
| 14 | `mkdir(path, path_len, mode)` | создаёт каталог | `EEXIST`, `ENOENT`, `EACCES`, `EROFS`, `ENOSPC` |
| 15 | `unlink(path, path_len)` | удаляет файл или пустой каталог | `ENOENT`, `ENOTEMPTY`, `EACCES`, `EROFS`, `EBUSY` (точка монтирования) |
| 16 | `rename(old, old_len, new, new_len)` | переименовывает или переносит; существующий файл (или пустой каталог) на новом месте заменяется | `ENOENT`, `EXDEV`, `EINVAL`, `ENOTEMPTY`, `EISDIR`, `ENOTDIR`, `EACCES`, `EBUSY` |
| 17 | `pipe(fds)` | создаёт канал (буфер 64 КиБ); пишет два `u64`: `fds[0]` — чтение, `fds[1]` — запись | `EMFILE`, `EFAULT` |
| 18 | `dup2(old, new)` | `new` указывает на тот же открытый файл, что `old` (прежний `new` закрывается) | `EBADF` |
| 19 | `chdir(path, path_len)` | меняет рабочий каталог (нужно право `x`) | `ENOENT`, `ENOTDIR`, `EACCES` |
| 20 | `kill(pid, status)` | завершает процесс с кодом `status`; root — любой, остальные — только свои | `ESRCH`, `EPERM` |
| 21 | `clock_gettime(clock, ts)` | `clock = 1` (MONOTONIC): время с загрузки; `clock = 0` (REALTIME, с этапа 6): время UTC с 1970 года по часам RTC; `struct { u64 sec; u64 nsec; }` | `EINVAL`, `EAGAIN` (время неизвестно), `EFAULT` |
| 22 | `sysinfo(out)` | пишет `SysInfo` (112 байт): время работы, память всего и занято, число процессов, время загрузки, число CPU, версия, модель CPU | `EFAULT` |
| 23 | `reboot(cmd)` | только root: `0` — перезагрузка, `1` — выключение (ACPI S5); перед этим данные ФС записываются на диски | `EPERM`, `EINVAL` |
| 24 | `setuid(uid, gid)` | только root: меняет пользователя и группу процесса (обратно не вернуться) | `EPERM`, `EINVAL` |
| 25 | `sleep_ms(ms)` | спит не меньше `ms` миллисекунд (точность — тик 10 мс) | — |
| 26 | `fstat(fd, out)` | `Stat` открытого файла; для консоли — символьное устройство | `EBADF`, `EFAULT` |
| 27 | `sync()` | записывает на диски всё, что файловые системы держат в памяти | `EIO` |
| 28 | `ftruncate(fd, size)` | меняет размер файла; новая часть читается как нули | `EBADF`, `EISDIR`, `EINVAL`, `ENOSPC` |
| 29 | `statfs(path, path_len, out)` | пишет `FsStat`: размер блока, блоки, свободные блоки, inode, тип ФС, только чтение | `ENOENT`, `EFAULT` |
| 32 | `getcwd(buf, len)` | пишет рабочий каталог (без NUL), возвращает длину | `EINVAL` (мал буфер), `EFAULT` |
| 33 | `getuid()` | `uid | gid << 32` | — |
| 34 | `proclist(buf, len)` | записи `ProcEntry` (72 байта): pid, родитель, uid, состояние, потоки, память, имя | `EFAULT` |
| 35 | `chmod(path, path_len, mode)` | права файла; владелец или root | `EPERM`, `EROFS`, `ENOENT` |
| 36 | `chown(path, path_len, uid, gid)` | владелец и группа; только root | `EPERM`, `EROFS`, `ENOENT` |
| 37 | `dup(fd)` | новый наименьший дескриптор на тот же открытый файл | `EBADF`, `EMFILE` |
| 38 | `tty(op, arg)` | консоль: `0` — режим (`RAW` 1, `NO_ECHO` 2), `1` — процесс, который завершает Ctrl+C, `2` — размер (`cols << 32 | rows`), `3` — размер ячейки в пикселях (`width << 32 | height`) | `EINVAL`, `ENODEV` (`3`: нет экрана) |
| 39 | `getrandom(buf, len)` | до 256 случайных байт (RDRAND, если есть, вместе с TSC) | `EFAULT` |
| 40 | `socket(kind)` | новый сокет: `1` — UDP, `2` — TCP; возвращает дескриптор | `EINVAL`, `ENETDOWN` (нет сети), `EMFILE` |
| 41 | `bind(fd, port)` | UDP: занять порт (0 — любой свободный); TCP: порт для `listen` | `EADDRINUSE`, `EINVAL` |
| 42 | `connect(fd, ip, port)` | TCP: соединиться и дождаться установки; UDP: адрес по умолчанию для `write` | `ECONNREFUSED`, `ETIMEDOUT`, `ENETUNREACH`, `EISCONN` |
| 43 | `listen(fd)` | TCP: принимать соединения на порту из `bind` | `EADDRINUSE`, `EINVAL` |
| 44 | `accept(fd)` | ждёт соединение, возвращает новый дескриптор | `EINVAL` |
| 45 | `sendto(fd, buf, len, ip, port)` | UDP: датаграмма на адрес (ip 0 — адрес из `connect`); TCP: данные соединения | `EDESTADDRREQ`, `ENOTCONN`, `EPIPE`, `ECONNRESET` |
| 46 | `recvfrom(fd, buf, len, from)` | ждёт данные; для UDP пишет отправителя (`SockAddr`); 0 — собеседник закрыл соединение | `ENOTCONN`, `ECONNRESET`, `ETIMEDOUT` |
| 47 | `netinfo(out)` | пишет `NetInfo` (48 байт): флаги, адрес, маска, шлюз, DNS, MAC, счётчики кадров | `EFAULT` |
| 48 | `ping(ip, seq, timeout_ms)` | эхо-запрос ICMP; возвращает время ответа в микросекундах | `ETIMEDOUT`, `ENETUNREACH`, `ENETDOWN` |
| 49 | `shutdown(fd)` | TCP: закрыть передачу (FIN), приём продолжается | `ENOTCONN` |

Неизвестный номер возвращает `-ENOSYS` (38).

### Файлы

- Относительный путь отсчитывается от рабочего каталога процесса
  (`chdir`, `getcwd`); так же ищется программа в `spawn`. `.` и `..`
  разбираются без обращения к диску; символических ссылок нет.
- Флаги `open`: режим доступа READ 0, WRITE 1, READ_WRITE 2; CREATE 0o100,
  EXCLUSIVE 0o200, TRUNCATE 0o1000, APPEND 0o2000, DIRECTORY 0o200000
  (значения как в Linux).
- Права проверяются при открытии и при изменении каталогов: нужен поиск
  (`x`) во всех каталогах пути и запись (`w`) в каталоге, который меняется.
  root (uid 0) может читать и писать всё, а запускать — только файлы с
  хотя бы одним битом `x`. Новый процесс получает пользователя, группу и
  рабочий каталог родителя.
- Новый процесс получает только дескрипторы 0, 1 и 2 родителя (на них
  shell подставляет каналы и файлы перенаправлений); остальные не
  наследуются. При завершении процесса все его файлы закрываются.

### Консоль (терминал)

- Строчный режим (по умолчанию): эхо, Backspace, строка отдаётся по Enter.
  Ctrl+D в пустой строке — конец ввода (`read` возвращает 0).
- `NO_ECHO`: без эха (ввод пароля).
- `RAW`: клавиши отдаются сразу, без эха; стрелки, Home, End, Delete —
  последовательности ANSI (`ESC [ A` и т. д.), Backspace — 0x7f. Так vsh
  сам редактирует строку.
- Ctrl+C в строчном режиме при заданном процессе переднего плана
  завершает его со статусом 130 сразу, даже если он не читает клавиатуру.
- Программы запускаются с любой файловой системы; нужен бит `x`
  (иначе `EACCES`).

### Коды ошибок

`EPERM 1, ENOENT 2, ESRCH 3, EINTR 4, EIO 5, E2BIG 7, ENOEXEC 8, EBADF 9,
ECHILD 10, EAGAIN 11, ENOMEM 12, EACCES 13, EFAULT 14, EBUSY 16, EEXIST 17,
EXDEV 18, ENOTDIR 20, EISDIR 21, EINVAL 22, EMFILE 24, EFBIG 27, ENOSPC 28,
ESPIPE 29, EROFS 30, ENAMETOOLONG 36, ENOSYS 38, ENOTEMPTY 39, EUCLEAN 117`
(значения как в Linux, для удобства переноса программ; `EUCLEAN` — файловая
система нашла повреждённые метаданные).

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
| 30 | `mount(source, target, type)` | позже (установщику не понадобился, см. D34) |
| 31 | `umount(target)` | позже |

Изменения фиксируются здесь и в [decisions.md](decisions.md).

## English

Status: the calls in the "Implemented" table work (stages 4–10). Stage 10
added the socket calls 40–49: `socket(kind)` (1 UDP, 2 TCP), `bind(fd, port)`,
`connect(fd, ip, port)` (TCP waits for the handshake), `listen(fd)`,
`accept(fd)`, `sendto(fd, buf, len, ip, port)`, `recvfrom(fd, buf, len, from)`
(0 = the peer closed), `netinfo(out)` (48-byte `NetInfo`), `ping(ip, seq,
timeout_ms)` (round trip in µs) and `shutdown(fd)` (TCP half-close). Sockets
are file descriptors, so `read`, `write` and `close` work on them. New error
numbers: ENOTSOCK 88, EDESTADDRREQ 89, EADDRINUSE 98, ENETDOWN 100,
ENETUNREACH 101, ECONNRESET 104, EISCONN 106, ENOTCONN 107, ETIMEDOUT 110,
ECONNREFUSED 111, EPIPE 32. Stage 9
added 39 `getrandom(buf, len)` (up to 256 random bytes: RDRAND when present,
mixed with the TSC). 30 `mount` and 31 `umount` stay reserved (decision D34). Numbers and
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
| 1 | `write(fd, buf, len)` | file: writes at the current position (at the end with `APPEND`), at most 1 MiB per call; console: at most 64 KiB, invalid UTF-8 shown as U+FFFD; returns the byte count | `EBADF`, `EFAULT`, `ENOSPC`, `EFBIG`, `EROFS`, `EIO` |
| 2 | `read(fd, buf, len)` | file: reads at the current position, at most 1 MiB, 0 at the end; console: waits for at least one character and returns whole UTF-8 characters (`len >= 4`) | `EBADF`, `EISDIR`, `EINVAL`, `EFAULT`, `EIO` |
| 3 | `yield()` | gives the CPU to other ready threads | — |
| 4 | `getpid()` | process id | — |
| 5 | `spawn(path, path_len, argv, argc, envp, envc)` | starts a program from a file; `argv`/`envp` are arrays of `StrRef { ptr, len }`; returns the child pid | `ENOENT`, `ENOEXEC`, `E2BIG`, `ENOMEM`, `EFAULT`, `EINVAL` |
| 6 | `wait(pid, status)` | waits for a child to exit and stores its status (`i64`) at `status` (may be 0); returns the pid | `ECHILD`, `EFAULT` |
| 7 | `mmap(addr, len, prot, flags)` | zero-filled anonymous memory; `prot`: READ 1, WRITE 2, EXEC 4 (WRITE+EXEC refused); `flags`: FIXED 1 — exactly at `addr` | `EINVAL`, `EEXIST`, `ENOMEM` |
| 8 | `munmap(addr, len)` | removes mappings and frees the pages | `EINVAL` |
| 9 | `open(path, path_len, flags, mode)` | opens a file and returns the lowest free descriptor (at most 64 per process); `mode` is the permission of a new file | `ENOENT`, `EEXIST`, `EACCES`, `EISDIR`, `ENOTDIR`, `EROFS`, `EMFILE`, `ENAMETOOLONG`, `EINVAL`, `EFAULT` |
| 10 | `close(fd)` | closes a descriptor | `EBADF` |
| 11 | `seek(fd, offset, whence)` | `whence`: SET 0, CURRENT 1, END 2; returns the new position | `EBADF`, `EINVAL`, `ESPIPE` (console) |
| 12 | `stat(path, path_len, out)` | writes a 64-byte `Stat`: dev, ino, mode (type and permissions), nlink, uid, gid, size, allocated, mtime, ctime | `ENOENT`, `ENOTDIR`, `EACCES`, `EFAULT` |
| 13 | `readdir(fd, buf, len)` | `DirentHeader` records (ino, rec_len, name_len, kind) followed by the name, 8-byte aligned; 0 at the end; the position is kept in the descriptor | `ENOTDIR`, `EINVAL` (record does not fit), `EFAULT` |
| 14 | `mkdir(path, path_len, mode)` | creates a directory | `EEXIST`, `ENOENT`, `EACCES`, `EROFS`, `ENOSPC` |
| 15 | `unlink(path, path_len)` | removes a file or an empty directory | `ENOENT`, `ENOTEMPTY`, `EACCES`, `EROFS`, `EBUSY` (mount point) |
| 16 | `rename(old, old_len, new, new_len)` | renames or moves; an existing file (or empty directory) at the target is replaced | `ENOENT`, `EXDEV`, `EINVAL`, `ENOTEMPTY`, `EISDIR`, `ENOTDIR`, `EACCES`, `EBUSY` |
| 21 | `clock_gettime(clock, ts)` | `clock = 1` (MONOTONIC): time since boot; `clock = 0` (REALTIME, since stage 6): UTC time since 1970 from the RTC; `struct { u64 sec; u64 nsec; }` | `EINVAL`, `EAGAIN` (time unknown), `EFAULT` |
| 25 | `sleep_ms(ms)` | sleeps at least `ms` milliseconds (10 ms tick resolution) | — |
| 26 | `fstat(fd, out)` | `Stat` of an open file; the console is a character device | `EBADF`, `EFAULT` |
| 27 | `sync()` | writes everything the file systems hold in memory to the disks | `EIO` |
| 28 | `ftruncate(fd, size)` | changes the file size; a new part reads as zeros | `EBADF`, `EISDIR`, `EINVAL`, `ENOSPC` |
| 29 | `statfs(path, path_len, out)` | writes `FsStat`: block size, blocks, free blocks, inodes, file system type, read-only | `ENOENT`, `EFAULT` |

Stage 8 added: 17 `pipe(fds)` (64 KiB buffer; writes two `u64`, read end
first), 18 `dup2(old, new)`, 19 `chdir(path, len)`, 20 `kill(pid, status)`
(root any process, others only their own), 22 `sysinfo(out)` (112-byte
`SysInfo`), 23 `reboot(cmd)` (root only: 0 restart, 1 power off via ACPI S5;
file system data is synced first), 24 `setuid(uid, gid)` (root only, one
way), 32 `getcwd(buf, len)`, 33 `getuid()` (`uid | gid << 32`), 34
`proclist(buf, len)` (72-byte `ProcEntry` records), 35 `chmod`, 36 `chown`
(root only), 37 `dup(fd)`, 38 `tty(op, arg)` (0 mode: `RAW` 1, `NO_ECHO` 2;
1 the process Ctrl+C ends; 2 size `cols << 32 | rows`; 3 cell size in
pixels `width << 32 | height`, `ENODEV` without a screen). The console has a
line mode (echo, Backspace, Enter delivers the line, Ctrl+D on an empty line
is the end of input), `NO_ECHO` for passwords, and a raw mode where keys
arrive at once and special keys as ANSI sequences (Backspace as 0x7f). In
line mode Ctrl+C ends the foreground process with status 130 even if it does
not read the keyboard. A new process gets only descriptors 0–2 of its parent,
plus its user, group and working directory.

Files: a relative path starts at the working directory (also for `spawn`). `.` and `..` are resolved without touching the
disk; there are no symbolic links. `open` flags: access mode READ 0, WRITE 1,
READ_WRITE 2; CREATE 0o100, EXCLUSIVE 0o200, TRUNCATE 0o1000, APPEND 0o2000,
DIRECTORY 0o200000 (Linux values). Permissions are checked on open and on
directory changes (search permission on every directory of the path, write
permission on the changed directory). Root (uid 0) may read and write
everything and run files with at least one `x` bit. All processes run as root
until `setuid` arrives in stage 8. Every process starts with descriptors 0, 1
and 2 on the console; its files are closed when it exits. Programs can be run
from any file system and need an `x` bit (otherwise `EACCES`).

An unknown number returns `-ENOSYS` (38). Error numbers match Linux to make
porting easier. A program that raises a CPU exception is reported and ends
with status `128 + vector` (page fault → 142, #GP → 141); the kernel keeps
running. Selectors, STAR/FMASK, personality and the planned calls are listed
in the Russian section above.
