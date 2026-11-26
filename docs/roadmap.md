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

Дальше / Later: xHCI и USB HID, SMP, гибридный ISO для USB, загрузка через BIOS,
Lumi, Win32-слой. / xHCI and USB HID, SMP, hybrid ISO for USB drives, BIOS boot,
Lumi, the Win32 layer.

## Цели производительности / Performance targets

| Цель / Target | Сейчас / Now (v0.3.0) |
|---|---|
| ОЗУ в простое < 64 МиБ / idle RAM < 64 MiB | 5,2 МиБ после загрузки (QEMU, 256 МиБ RAM); проверяется тестом загрузки / 5.2 MiB after boot (QEMU, 256 MiB RAM); checked by the boot test |
| Загрузка до приглашения входа < 3 с в QEMU / boot to login prompt < 3 s in QEMU | ядро загружается за 0,1 с (включая калибровку таймера 50 мс); всего с прошивкой 2,8–4,8 с на TCG в Windows, с KVM в CI 2,3 с; приглашения входа пока нет / the kernel boots in 0.1 s (including the 50 ms timer calibration); with firmware 2.8–4.8 s on TCG under Windows, 2.3 s with KVM in CI; no login prompt yet |
