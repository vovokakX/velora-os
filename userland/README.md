# userland

Пользовательские программы. Отдельный workspace: программы собираются с
другими флагами, чем ядро (нижняя половина адресов, статическая модель
перемещений, скрипт компоновки `link.ld`). Сборку запускает `build.sh`, а
программы попадают в initramfs (`/bin/...`).

| Программа | Назначение |
|---|---|
| `login` | `/bin/login`: вход по `/etc/passwd` и `/etc/shadow`, запуск оболочки |
| `vsh` | `/bin/vsh`: оболочка; язык и редактор строки — в `libc/shell` |
| `coreutils` | команды `ls`, `cat`, `cp`, `mv`, `rm`, `mkdir`, `touch`, `echo`, `clear`, `ps`, `kill`, `mem`, `uptime`, `date`, `whoami`, `reboot`, `shutdown`, `help`, `velofetch` (по файлу в `src/bin`) |
| `hello` | `/bin/hello`: печатает pid, аргументы и окружение |
| `vtest` | `/bin/vtest`: проверки vlibc и системных вызовов из пространства пользователя (запускается тестами ядра) |

`rootfs/` — обычные файлы, которые тоже кладутся в initramfs (`/etc/passwd`,
`/etc/group`, `/etc/motd`). `/etc/shadow` создаётся при сборке
(`tools/mkpasswd`).

---

User programs, in a separate workspace because they are built with different
flags than the kernel (lower-half addresses, static relocation model, the
`link.ld` linker script). `build.sh` builds them into the initramfs (`/bin/...`):
`login` checks `/etc/passwd` and `/etc/shadow` and starts the shell; `vsh` is
the shell (its language and line editor are in `libc/shell`); `coreutils` holds
the commands, one file per command in `src/bin`; `hello` prints its pid,
arguments and environment; `vtest` checks vlibc and the system calls from user
space (run by the kernel tests). `rootfs/` holds plain files that also go into
the initramfs (`/etc/passwd`, `/etc/group`, `/etc/motd`); `/etc/shadow` is
generated at build time (`tools/mkpasswd`).
