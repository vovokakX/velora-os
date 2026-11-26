# Системные вызовы / System calls

[Русский](#русский) | [English](#english)

## Русский

Состояние: **не реализовано**. Системные вызовы появятся на этапе 4. Здесь
описано соглашение, под которое уже подготовлены GDT и архитектура.

### Соглашение о вызове (нативный Velora ABI)

- Инструкция `syscall`. Номер вызова в `rax`.
- Аргументы: `rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9` (`r10` вместо `rcx`,
  потому что `syscall` записывает в `rcx` адрес возврата).
- Результат в `rax`: значение `>= 0` — успех, отрицательное — код ошибки
  (`-errno`).
- `rcx` и `r11` портятся инструкцией `syscall`. Остальные регистры
  сохраняются.

### Селекторы (из `kernel/src/arch/x86_64/gdt.rs`)

| Селектор | Значение |
|---|---|
| 0x08 | код ядра |
| 0x10 | данные ядра |
| 0x18 | код пользователя, 32 бита (нужен только для раскладки `sysret`) |
| 0x23 | данные пользователя |
| 0x2b | код пользователя, 64 бита |
| 0x30 | TSS |

`STAR[47:32] = 0x08`, `STAR[63:48] = 0x18`.

### Personality

Номер вызова ищется в таблице той personality, к которой относится процесс.
Нативные программы используют таблицу Velora. Программы Win32 (в будущем)
не вызывают ядро напрямую: это делает Win32-слой в пространстве
пользователя через нативную таблицу.

### Планируемый список (этапы 4–8)

| № | Имя | Этап |
|---|---|---|
| 0 | `exit(code)` | 4 |
| 1 | `write(fd, buf, len)` | 4 |
| 2 | `read(fd, buf, len)` | 4 |
| 3 | `yield()` | 4 |
| 4 | `getpid()` | 4 |
| 5 | `spawn(path, argv, envp)` | 5 |
| 6 | `wait(pid, status)` | 5 |
| 7 | `mmap(addr, len, prot, flags)` | 5 |
| 8 | `munmap(addr, len)` | 5 |
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
| 21 | `clock_gettime(clock, ts)` | 6 |
| 22 | `sysinfo(buf)` | 8 |
| 23 | `reboot(cmd)` | 8 |
| 24 | `setuid(uid)` / `getuid()` | 8 |

Список может меняться до этапа 8. Изменения фиксируются здесь и в
[decisions.md](decisions.md).

## English

Status: **not implemented**. System calls arrive in stage 4. This page
describes the convention the GDT and the architecture are already prepared
for.

### Calling convention (native Velora ABI)

- `syscall` instruction, call number in `rax`.
- Arguments in `rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9` (`r10` replaces `rcx`
  because `syscall` stores the return address in `rcx`).
- Result in `rax`: `>= 0` on success, negative error code (`-errno`) on failure.
- `rcx` and `r11` are clobbered by `syscall`; all other registers are preserved.

### Selectors

See the table above (from `kernel/src/arch/x86_64/gdt.rs`).
`STAR[47:32] = 0x08`, `STAR[63:48] = 0x18`.

### Personality

The call number is looked up in the table of the process's personality.
Native programs use the Velora table. Future Win32 programs do not call the
kernel directly; the Win32 layer in user space does it through the native
table.

### Planned calls

See the table above. The list may change until stage 8; changes are recorded
here and in [decisions.md](decisions.md).
