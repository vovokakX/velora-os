# План этапов / Roadmap

Каждый этап: своя ветка, issue, Pull Request с зелёным CI, тег и релиз.
Each stage: its own branch, issue, pull request with green CI, tag and release.

| # | Ветка / Branch | Содержание / Scope | Критерий готовности / Done when | Версия |
|---|---|---|---|---|
| 1 | `stage-1-boot` | Загрузка UEFI, framebuffer и serial, исключения, паника, сборка ISO, CI / UEFI boot, framebuffer and serial, exceptions, panic, ISO build, CI | Тесты ядра в QEMU и тест загрузки проходят / kernel tests and boot test pass in QEMU | v0.1.0 |
| 2 | `stage-2-memory` | Физический аллокатор, свои таблицы страниц с правами W^X, куча ядра, теневой буфер консоли / physical allocator, own page tables with W^X, kernel heap, console back buffer | Тесты аллокаторов, нагрузочный тест кучи, page fault при записи в `.text` / allocator tests, heap stress test, page fault on write to `.text` | v0.2.0 |
| 3 | `stage-3-interrupts` | ACPI (MADT), LAPIC, IOAPIC, таймер, клавиатура PS/2 / ACPI (MADT), LAPIC, IOAPIC, timer, PS/2 keyboard | Тики таймера, ввод с клавиатуры через QEMU / timer ticks, keyboard input via QEMU | v0.3.0 |
| 4 | `stage-4-processes` | Потоки, вытесняющий планировщик, ring 3, `syscall`/`sysret`, personality / threads, preemptive scheduler, ring 3, syscalls, personality | Несколько процессов ring 3 по очереди, изоляция памяти / several ring 3 processes take turns, memory isolation | v0.4.0 |
| 5 | `stage-5-exec` | ELF, `ExecFormat`, заготовка PE, initramfs, база vlibc / ELF, `ExecFormat`, PE stub, initramfs, vlibc basics | Запуск ELF из initramfs / ELF programs run from initramfs | v0.5.0 |
| 6 | `stage-6-drivers` | PCI, virtio-blk, AHCI, NVMe, RTC | Чтение и запись секторов на всех трёх контроллерах в QEMU / sector read/write on all three controllers in QEMU | v0.6.0 |
| 7 | `stage-7-fs` | VFS, FAT32, VeloraFS (права, журнал, контрольные суммы) / VFS, FAT32, VeloraFS (permissions, journal, checksums) | Файлы сохраняются после перезагрузки, восстановление после сбоя / files persist across reboot, crash recovery | v0.7.0 |
| 8 | `stage-8-userland` | vsh, команды, пользователи, login, velofetch | Сценарий входа и команд в QEMU / login and command scenario in QEMU | v0.8.0 |
| 9 | `stage-9-installer` | velora-setup: GPT, форматирование, копирование, загрузчик / GPT, formatting, copy, bootloader | Полный E2E: установка, перезагрузка, вход, файлы / full E2E: install, reboot, login, files | v0.9.0 |
| 10 | `stage-10-network` | virtio-net, ARP, IPv4, ICMP, UDP, DHCP, TCP | ping и TCP-обмен с хостом QEMU / ping and TCP exchange with the QEMU host | v0.10.0 |
| 11 | `stage-11-plume` | Текстовый редактор Plume (уровня nano) / Plume text editor (nano level) | Тест `boot` создаёт, ищет, вырезает, вставляет и сохраняет файл с кириллицей / the `boot` test creates, searches, cuts, pastes and saves a file with Cyrillic text | v0.11.0 |
| 12 | `stage-12-hardware` | Консоль GOP со шрифтом с кириллицей, таймеры HPET/TSC, журнал паники на диске, образ `velora-usb.img`, `docs/hardware.md` / GOP console with a Cyrillic font, HPET/TSC timers, panic log on disk, `velora-usb.img`, `docs/hardware.md` | Загрузка образа для флешки в QEMU, калибровка без PIT, журнал паники читается после перезагрузки / the USB image boots in QEMU, calibration without the PIT, the panic log is readable after a reboot | v0.12.0 |
| 13 | `stage-13-acpi` | Выключение и перезагрузка через ACPI, кнопка питания, AML / ACPI power off and restart, power button, AML | Выключение и кнопка питания в QEMU / power off and the power button in QEMU | v0.13.0 |
| 14 | `stage-14-smp` | Работа на всех ядрах процессора / all CPU cores in use | Тесты в QEMU с `-smp 4` / tests in QEMU with `-smp 4` | v0.14.0 |
| 15 | `stage-15-usb` | xHCI, клавиатура и мышь USB HID, флешки (FAT32) / xHCI, USB HID keyboard and mouse, flash drives (FAT32) | Ввод и файлы через USB-устройства QEMU / input and files through QEMU USB devices | v0.15.0 |
| 16 | `stage-16-net` | e1000, DHCP, DNS, `ping`, `fetch` (HTTP) / e1000, DHCP, DNS, `ping`, `fetch` (HTTP) | Скачивание по HTTP с хоста QEMU по имени / an HTTP download from the QEMU host by name | v0.16.0 |

Дальше / Later: загрузка через BIOS, Lumi, Win32-слой. / BIOS boot, Lumi,
the Win32 layer.

## Цели производительности / Performance targets

| Цель / Target | Сейчас / Now (v0.11.0) |
|---|---|
| ОЗУ в простое < 64 МиБ / idle RAM < 64 MiB | 8,0 МиБ после загрузки Live-образа, включая initramfs с файлами для установки (QEMU, 256 МиБ RAM); проверяется тестом загрузки / 8.0 MiB after booting the Live image, initramfs with the files to install included, initramfs included (QEMU, 256 MiB RAM); checked by the boot test |
| Загрузка до приглашения входа < 3 с в QEMU / boot to login prompt < 3 s in QEMU | приглашение входа через 1,85 с с KVM в CI (QEMU 9.1, прошивка включена), у установленной системы 1,7 с; на TCG в Windows 3,2–5,2 с; время измеряют тесты загрузки и установки / login prompt after 1.85 s with KVM in CI (QEMU 9.1, firmware included), 1.7 s for an installed system; 3.2–5.2 s on TCG under Windows; measured by the boot and installation tests |
