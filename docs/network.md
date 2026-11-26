# Сеть / Network

[Русский](#русский) | [English](#english)

## Русский

### Устройство

- Драйвер `kernel/src/drivers/virtio_net.rs`: современный virtio-net
  (virtio 1.x), одна очередь приёма и одна передачи по 64 буфера на
  2 КиБ, без аппаратных разгрузок. MAC-адрес берётся у устройства.
- Стек `kernel/net` (crate `velora-net`, `no_std`): Ethernet, ARP, IPv4,
  ICMP echo, UDP, клиент DHCP, TCP. Стек сам не делает ввода-вывода.
  Драйвер передаёт ему принятые кадры (`receive`), вызывает таймеры
  (`poll`) и отправляет кадры, которые стек выдаёт (`take_frame`). Поэтому
  весь стек проверяется на хосте: в тестах два стека обмениваются кадрами
  напрямую, с потерями и без.
- Поток ядра `net` переносит кадры между устройством и стеком. Он
  просыпается по прерыванию MSI-X и не реже раза в тик (10 мс).
- При загрузке ядро запрашивает адрес по DHCP. Когда адрес получен,
  печатается строка `net0: 10.0.2.15/24 via 10.0.2.2 (DHCP)`.
- Сокеты — файловые дескрипторы: `socket`, `bind`, `listen`, `accept`,
  `connect`, `sendto`, `recvfrom`, `shutdown`, а также `read`/`write` и
  `close`. Есть ещё `netinfo` и `ping` (`docs/syscalls.md`).
- vlibc: `net::TcpStream`, `TcpListener`, `UdpSocket`, `netinfo`, `ping`.
- Команды: `ip` (адрес интерфейса), `ping [-c N] АДРЕС`,
  `nc АДРЕС ПОРТ` (отправить стандартный ввод и вывести ответ) и
  `nc -l ПОРТ [ОТВЕТ]`.

### TCP

Есть: установка соединения (в том числе ответная сторона, `listen` и
`accept`), опция MSS, окно получателя, подтверждения, повторная передача
по таймеру с удвоением интервала (от 0,5 до 8 с) и быстрая повторная
передача после трёх одинаковых подтверждений, закрытие в обе стороны и
полузакрытие (`shutdown`), RST. Буферы — 64 КиБ на приём и на передачу.

NOT IMPLEMENTED: сборка сегментов, пришедших не по порядку (они
отбрасываются, отправитель повторяет их), управление перегрузкой, масштаб
окна, SACK, пробы нулевого окна, keep-alive. TIME-WAIT длится 2 с.

### Ограничения

- Одна сетевая карта (первая virtio-net), только IPv4.
- Нет DNS: адреса указываются числами.
- Аренда DHCP не продлевается.
- Нет фрагментации IP (фрагменты отбрасываются).
- Нет `select`/`poll` и неблокирующих сокетов.

### Как проверено

- Юнит-тесты `velora-net` на хосте: контрольные суммы (пример из
  RFC 1071), заголовки, ARP и ping, обмен UDP, передача 200 КБ по TCP,
  полузакрытие, потеря около 20 % кадров, отказ (RST) и таймаут, нет
  маршрута, DHCP с имитацией сервера и повтор запроса.
- Тест ядра в QEMU (`-netdev user`): DHCP выдаёт 10.0.2.15, шлюз 10.0.2.2
  отвечает на ping.
- `python tests/qemu_run.py net`: вход в систему, `ip`, `ping -c 3`
  шлюза, `nc` к TCP-серверу на хосте (данные доходят, ответ приходит),
  соединение с хоста к `nc -l` в гостевой системе через `hostfwd`,
  соединение к закрытому порту. Сеть QEMU (slirp) в этом случае не
  отвечает RST, поэтому соединение завершается по таймауту; ответ RST
  проверяется юнит-тестами.

## English

**Design.** The driver (`kernel/src/drivers/virtio_net.rs`) handles modern
virtio-net (virtio 1.x) with one receive and one transmit queue of 64 2-KiB
buffers and no offloads; the MAC address comes from the device. The stack
(`kernel/net`, crate `velora-net`, `no_std`) covers Ethernet, ARP, IPv4, ICMP
echo, UDP, a DHCP client and TCP. It does no I/O itself: the driver feeds it
received frames (`receive`), runs its timers (`poll`) and sends the frames it
produces (`take_frame`), so the whole stack is tested on the host with two
stacks exchanging frames directly, with and without loss. The `net` kernel
thread moves frames between the device and the stack; it wakes on the MSI-X
interrupt and at least every tick (10 ms). At boot the kernel asks for an
address with DHCP and prints `net0: 10.0.2.15/24 via 10.0.2.2 (DHCP)` once it
has one. Sockets are file descriptors (`socket`, `bind`, `listen`, `accept`,
`connect`, `sendto`, `recvfrom`, `shutdown`, plus `read`/`write`/`close`);
`netinfo` and `ping` complete the calls (`docs/syscalls.md`). vlibc offers
`net::TcpStream`, `TcpListener`, `UdpSocket`, `netinfo`, `ping`. Commands:
`ip`, `ping [-c N] ADDRESS`, `nc ADDRESS PORT` (send standard input, print the
answer) and `nc -l PORT [REPLY]`.

**TCP** has connection setup (including the passive side, `listen`/`accept`),
the MSS option, the receiver's window, acknowledgements, retransmission on a
timer with doubling intervals (0.5 to 8 s) and fast retransmission after three
duplicate ACKs, closing in both directions and half-close (`shutdown`), RST.
Buffers are 64 KiB each way. NOT IMPLEMENTED: reassembly of out-of-order
segments (they are dropped and the sender repeats them), congestion control,
window scaling, SACK, zero-window probes, keep-alive. TIME-WAIT lasts 2 s.

**Limits:** one card (the first virtio-net), IPv4 only, no DNS (numeric
addresses), no DHCP lease renewal, no IP fragmentation, no `select`/`poll` or
non-blocking sockets.

**Tests:** host unit tests (checksums with the RFC 1071 example, headers, ARP
and ping, UDP, a 200 KB TCP transfer, half-close, about 20 % frame loss, RST
refusal and timeout, no route, DHCP against a simulated server and request
retries); a kernel test in QEMU (`-netdev user`: DHCP gives 10.0.2.15, the
gateway 10.0.2.2 answers ping); `python tests/qemu_run.py net` (login, `ip`,
`ping -c 3` to the gateway, `nc` to a TCP server on the host, a connection from
the host to `nc -l` in the guest through `hostfwd`, and a closed port, which
QEMU's slirp network lets time out instead of answering RST; RST handling is
covered by the unit tests).
