# velora-setup

[Русский](#русский) | [English](#english)

## Русский

Установщик Velora OS. Запускается от root в Live-режиме (загрузка с ISO) и
устанавливает систему на диск. Программа: `/bin/velora-setup`, код:
`installer/velora-setup` (собирается вместе с программами из `userland/`).

### Стартовое меню и графический установщик

При загрузке с ISO или флешки сначала открывается меню (`/bin/velora-start`):
1. Графическая установка (по умолчанию, начинается через 10 с).
2. Установка в терминале — `velora-setup` (ниже).
3. Live-режим — вход в систему (пароль `velora`) и терминал vsh.

Графический установщик: приветствие и язык установщика, язык и регион,
клавиатура и курсор, оформление, разметка диска (автоматическая, ручная,
«Эксперт»), пользователь, подтверждение, установка, готово. Управление
мышью и клавиатурой (Tab, стрелки, Enter, Esc — назад). Из Live-системы его
можно открыть командой `velora-start --install` (от root). Обе программы
используют одно ядро установщика (`installer/core`, crate `velora-install`);
экраны — `installer/gui`. Выбор оформления, языка и раскладки записывается в
`/etc/velora/desktop.conf`, `locale.conf` и `keyboard.conf` для будущей
оболочки; система эти файлы пока не читает.

### Запуск

```
velora-setup
```

Установщик показывает диски и спрашивает:
- диск для установки;
- имя нового пользователя (строчные латинские буквы, цифры, `_`, `-`);
- пароль пользователя и пароль root (дважды, без эха);
- подтверждение: нужно ввести `yes`, иначе диск не меняется.

После установки: извлечь ISO и выполнить `reboot`.

Без вопросов (для тестов):

```
SETUP_ROOT_PASSWORD=... SETUP_USER_PASSWORD=... \
    velora-setup --auto --disk vd0 --user anna --yes
```

### Что делает

1. Записывает новую таблицу GPT (всё прежнее содержимое диска теряется):
   - раздел 1 — EFI system partition, FAT32, 64 МиБ, с 1 МиБ;
   - раздел 2 — корень Velora (тип `5E1F7A30-9C42-4D8B-A1E7-56454C4F5241`),
     VeloraFS, до конца диска.
2. Форматирует раздел EFI и кладёт туда `/EFI/BOOT/BOOTX64.EFI` (Limine),
   `/boot/velora` (ядро без отладочных данных) и `/boot/limine.conf` с
   параметром `velora.root=PARTUUID=<GUID корневого раздела>`.
3. Форматирует корневой раздел (VeloraFS) и копирует программы из `/bin`
   (кроме `velora-setup` и `vtest`) и `/etc/motd`.
4. Создаёт `/etc/passwd`, `/etc/group`, `/etc/shadow` (хеши PBKDF2 с новой
   солью) и домашний каталог пользователя.
5. Проверяет новую файловую систему (fsck) и записывает всё на диск.

Файловые системы пишутся через `/dev/<диск>` теми же библиотеками, что
использует ядро (`velora-gpt`, `velora-fat`, `velorafs`); ничего не
монтируется. Диск, который смонтирован в `/mnt`, установщик не трогает.

Требования: диск не меньше 128 МиБ, загрузка через UEFI.

## English

The Velora OS installer. It runs as root in the Live system (booted from the
ISO) and installs the system onto a disk. Program: `/bin/velora-setup`; code:
`installer/velora-setup` (built with the programs in `userland/`).

Booting the ISO or a USB drive first opens a menu (`/bin/velora-start`):
1) graphical installation (the default, starts after 10 s), 2) installation in
the terminal (`velora-setup`, below), 3) the Live system (log in with the
password `velora`, the vsh terminal). The graphical installer has the steps
welcome and installer language, language and region, keyboard and pointer,
appearance, disk (automatic, manual, expert), user, confirmation,
installation and done; it works with the mouse and the keyboard (Tab, arrows,
Enter, Escape goes back) and can be opened from the Live system with
`velora-start --install` (as root). Both installers use one core
(`installer/core`, crate `velora-install`); the screens are in
`installer/gui`. Appearance, language and layout choices are written to
`/etc/velora/desktop.conf`, `locale.conf` and `keyboard.conf` for the future
shell; the system does not read them yet.

Run `velora-setup`: it lists the disks and asks for the target disk, the name
of the new user (lowercase letters, digits, `_`, `-`), the user's and root's
passwords (twice, without echo) and a confirmation (type `yes`, anything else
leaves the disk unchanged). Afterwards remove the ISO and run `reboot`.
Unattended (for tests): `SETUP_ROOT_PASSWORD=... SETUP_USER_PASSWORD=...
velora-setup --auto --disk vd0 --user anna --yes`.

What it does: (1) writes a new GPT (everything on the disk is lost): partition
1 is the EFI system partition (FAT32, 64 MiB, starting at 1 MiB), partition 2
the Velora root (type `5E1F7A30-9C42-4D8B-A1E7-56454C4F5241`, VeloraFS, to the
end of the disk); (2) formats the EFI partition and writes
`/EFI/BOOT/BOOTX64.EFI` (Limine), `/boot/velora` (the kernel without debug
data) and `/boot/limine.conf` with `velora.root=PARTUUID=<root GUID>`; (3)
formats the root (VeloraFS) and copies the programs from `/bin` (except
`velora-setup` and `vtest`) and `/etc/motd`; (4) creates `/etc/passwd`,
`/etc/group`, `/etc/shadow` (PBKDF2 hashes with fresh salts) and the user's
home directory; (5) checks the new file system (fsck) and syncs.

The file systems are written through `/dev/<disk>` with the same libraries the
kernel uses (`velora-gpt`, `velora-fat`, `velorafs`); nothing is mounted. A disk
mounted under `/mnt` is refused. Requirements: at least 128 MiB, UEFI boot.
