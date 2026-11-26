# userland

Пользовательские программы. Отдельный workspace: программы собираются с
другими флагами, чем ядро (нижняя половина адресов, статическая модель
перемещений, скрипт компоновки `link.ld`). Сборку запускает `build.sh`, а
программы попадают в initramfs (`/bin/...`).

| Программа | Назначение |
|---|---|
| `hello` | `/bin/hello`: печатает pid, аргументы и окружение; запускается при загрузке |
| `vtest` | `/bin/vtest`: проверки vlibc и системных вызовов из пространства пользователя (запускается тестами ядра) |

`rootfs/` — обычные файлы, которые тоже кладутся в initramfs.

Оболочка vsh и команды появятся на этапе 8.

---

User programs, in a separate workspace because they are built with different
flags than the kernel (lower-half addresses, static relocation model, the
`link.ld` linker script). `build.sh` builds them into the initramfs (`/bin/...`):
`hello` prints its pid, arguments and environment and runs at boot; `vtest`
checks vlibc and the system calls from user space (run by the kernel tests).
`rootfs/` holds plain files that also go into the initramfs. The vsh shell and
commands arrive in stage 8.
