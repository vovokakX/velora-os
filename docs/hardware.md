# Запуск на реальном компьютере / Running on a real computer

[Русский](#русский) | [English](#english)

## Русский

### Что нужно

- Процессор x86_64 (64-битный Intel или AMD).
- Прошивка UEFI. Режим BIOS (Legacy, CSM) не поддерживается.
- Secure Boot выключен: загрузчик Limine и ядро не подписаны.
- 256 МиБ памяти или больше (после загрузки занято около 8 МиБ).
- Флешка на 128 МиБ или больше. Всё содержимое флешки будет стёрто.

### Образ для флешки

Сборка (`./build.sh`) создаёт два образа:

| Файл | Для чего |
|---|---|
| `velora.iso` | CD/DVD и виртуальные машины (QEMU, VirtualBox) |
| `velora-usb.img` | запись на флешку «как есть», побайтно |

`velora-usb.img` — диск с таблицей разделов GPT и одним разделом EFI
(FAT32). На нём: загрузчик `EFI/BOOT/BOOTX64.EFI` (Limine), настройки
`boot/limine.conf`, ядро `boot/velora` и `boot/initramfs.tar`. Образ
есть и в каждом релизе на GitHub, рядом с контрольной суммой SHA-256.

### Запись на флешку

Сначала проверьте контрольную сумму: `sha256sum -c velora-usb.img.sha256`.
Запись стирает выбранный диск целиком, поэтому выбирайте его очень внимательно.

- Windows: [Rufus](https://rufus.ie) в режиме «DD-образ» или
  balenaEtcher. Выберите флешку и файл `velora-usb.img`.
- Linux: узнайте имя флешки (`lsblk`), затем
  `sudo dd if=velora-usb.img of=/dev/sdX bs=4M conv=fsync status=progress`,
  где `/dev/sdX` — флешка, а не раздел и не системный диск.
- macOS: `diskutil list`, `diskutil unmountDisk /dev/diskN`, затем
  `sudo dd if=velora-usb.img of=/dev/rdiskN bs=4m`.

Раздел на флешке — обычный FAT32. Его можно открыть в любой системе и,
например, поправить `boot/limine.conf`.

### Загрузка

1. В настройках прошивки выключите Secure Boot. Если есть выбор режима,
   выберите UEFI (не Legacy/CSM).
2. Вставьте флешку и выберите её в меню загрузки (обычно F12, F11, F8 или
   Esc при включении).
3. Через несколько секунд появится приглашение входа. Пользователь `user`
   или `root`, пароль `velora`.
4. Установка на внутренний диск: войдите как `root` и запустите
   `velora-setup` (см. [installer/README.md](../installer/README.md)).
   Установщик стирает выбранный диск целиком.

### Что поддерживается (v0.15.0)

| Устройство | Состояние |
|---|---|
| Экран | framebuffer UEFI GOP, 32 бита на пиксель; шрифт 8×16 с кириллицей, на экранах от 2560 точек — увеличенный вдвое |
| Клавиатура | PS/2 (обычно встроенная клавиатура ноутбука) и USB (HID boot protocol) на корневых портах контроллера xHCI |
| Диски | SATA (AHCI), NVMe, virtio-blk |
| Таймеры | HPET, таймер ACPI PM или PIT для калибровки; TSC и таймер APIC |
| Сеть | только virtio-net (виртуальные машины). Сетевые карты Intel — с этапа 16 |
| Процессоры | все ядра (до 64): программы работают параллельно, код ядра — под общей блокировкой |
| Питание | выключение и перезагрузка через ACPI; кнопка питания на ПК, где она фиксированное событие ACPI (обычно настольные ПК). У многих ноутбуков кнопка — метод AML, она не работает: выключайте командой `shutdown` |
| USB | контроллеры xHCI: клавиатура, мышь (`/dev/mouse`), флешки (разделы, FAT32). Только устройства на корневых портах, подключённые при загрузке: хабы и горячее подключение не поддерживаются |

### Если что-то пошло не так

- **Журнал паники.** При панике ядро записывает отчёт и последние
  сообщения ядра в файл `velora-panic.log` на разделе EFI, с которого
  загрузилась система. Это работает и для установленной системы, и при
  загрузке с флешки. При следующей загрузке ядро переносит журнал в
  `velora-panic.prev.log` и сообщает об этом. Журнал можно открыть на
  другом компьютере: это обычный текстовый файл на FAT32. Чтобы писать
  журнал на любой другой том FAT, создайте на нём пустой файл
  `velora-panic.log`.
- **Сообщения ядра.** Команда `dmesg` показывает последние 64 КиБ сообщений
  ядра. Сохранить их: `dmesg > /mnt/<диск>/dmesg.txt`.
- **Последовательный порт.** Если у компьютера есть COM1, все сообщения
  идут туда (115200 бод, 8N1).
- **Параметры ядра** задаются в строке `cmdline:` файла `boot/limine.conf`:

| Параметр | Значение |
|---|---|
| `velora.fontscale=1` (`2`, `3`) | размер шрифта консоли; без параметра выбирается сам |
| `velora.root=PARTUUID=<GUID>` | корневой раздел VeloraFS (ставит установщик) |

Сообщения о проблемах — в issues репозитория. Приложите `velora-panic.log`
или вывод `dmesg` и укажите модель компьютера.

## English

### Requirements

- An x86_64 processor (64-bit Intel or AMD).
- UEFI firmware. BIOS mode (Legacy, CSM) is not supported.
- Secure Boot turned off: the Limine bootloader and the kernel are not signed.
- 256 MiB of memory or more (about 8 MiB are in use after boot).
- A USB drive of 128 MiB or more. Everything on it is erased.

### The USB image

The build (`./build.sh`) makes two images:

| File | Use |
|---|---|
| `velora.iso` | CD/DVD and virtual machines (QEMU, VirtualBox) |
| `velora-usb.img` | written to a USB drive as is, byte for byte |

`velora-usb.img` is a disk with a GPT partition table and one EFI system
partition (FAT32) holding the bootloader `EFI/BOOT/BOOTX64.EFI` (Limine), its
settings `boot/limine.conf`, the kernel `boot/velora` and
`boot/initramfs.tar`. Every GitHub release has it, next to its SHA-256
checksum.

### Writing the drive

Check the checksum first: `sha256sum -c velora-usb.img.sha256`. Writing erases
the whole chosen disk, so choose it very carefully.

- Windows: [Rufus](https://rufus.ie) in "DD image" mode, or balenaEtcher.
  Pick the USB drive and `velora-usb.img`.
- Linux: find the drive (`lsblk`), then
  `sudo dd if=velora-usb.img of=/dev/sdX bs=4M conv=fsync status=progress`,
  where `/dev/sdX` is the USB drive, not a partition and not the system disk.
- macOS: `diskutil list`, `diskutil unmountDisk /dev/diskN`, then
  `sudo dd if=velora-usb.img of=/dev/rdiskN bs=4m`.

The partition on the drive is plain FAT32; any system can open it, for
example to edit `boot/limine.conf`.

### Booting

1. In the firmware settings turn Secure Boot off. If there is a boot mode
   choice, pick UEFI (not Legacy/CSM).
2. Plug the drive in and choose it in the boot menu (usually F12, F11, F8 or
   Esc at power-on).
3. The login prompt appears after a few seconds. User `user` or `root`,
   password `velora`.
4. To install on an internal disk, log in as `root` and run `velora-setup`
   (see [installer/README.md](../installer/README.md)). The installer erases
   the whole chosen disk.

### What is supported (v0.15.0)

| Device | State |
|---|---|
| Screen | UEFI GOP framebuffer, 32 bits per pixel; 8×16 font with Cyrillic, doubled on screens 2560 pixels wide or more |
| Keyboard | PS/2 (usually a laptop's built-in keyboard) and USB (HID boot protocol) on xHCI root ports |
| Disks | SATA (AHCI), NVMe, virtio-blk |
| Timers | HPET, the ACPI PM timer or the PIT for calibration; the TSC and the APIC timer |
| Network | virtio-net only (virtual machines). Intel network cards from stage 16 |
| Processors | all cores (up to 64): programs run in parallel, kernel code under one lock |
| Power | ACPI power off and restart; the power button on PCs where it is a fixed ACPI event (usually desktops). On many laptops the button is an AML method and does not work: use `shutdown` |
| USB | xHCI controllers: keyboard, mouse (`/dev/mouse`), flash drives (partitions, FAT32). Only devices on root ports present at boot: no hubs, no hot plugging |

### When something goes wrong

- **Panic log.** On a panic the kernel writes the report and the latest
  kernel messages to `velora-panic.log` on the EFI partition the system booted
  from. This works for an installed system and when booting from a USB drive. At the next boot the
  kernel moves the log to `velora-panic.prev.log` and says so. The log can be
  read on another computer: it is a plain text file on FAT32. To keep the log
  on any other FAT volume, create an empty `velora-panic.log` on it.
- **Kernel messages.** `dmesg` shows the last 64 KiB of kernel messages; save
  them with `dmesg > /mnt/<disk>/dmesg.txt`.
- **Serial port.** If the computer has COM1, all messages go there too
  (115200 baud, 8N1).
- **Kernel parameters** go on the `cmdline:` line of `boot/limine.conf`:

| Parameter | Meaning |
|---|---|
| `velora.fontscale=1` (`2`, `3`) | console font size; chosen automatically when absent |
| `velora.root=PARTUUID=<GUID>` | VeloraFS root partition (set by the installer) |

Report problems in the repository's issues; attach `velora-panic.log` or the
`dmesg` output and name the computer model.
