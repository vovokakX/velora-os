# VeloraFS

[Русский](#русский) | [English](#english)

## Русский

VeloraFS — собственная файловая система Velora OS для корня установленной
системы. Код: `fs/velorafs` (crate `velorafs`, `no_std` + `alloc`). Он
собирается и в ядро, и в программы для хоста (`velora-fsck`) и проверяется
юнит-тестами на хосте.

### Свойства

- Блоки по 4 КиБ. Сектор устройства может быть любым делителем 4096.
- Файлы хранятся экстентами (начало, длина). 10 экстентов лежат в inode,
  ещё 255 — в одном дополнительном блоке. Незаписанные участки файла
  (дыры) читаются как нули и места не занимают.
- У каждого inode есть владелец, группа и биты прав POSIX (`rwx` для
  владельца, группы и остальных, а также setuid/setgid/sticky).
- Журнал метаданных. Каждая операция — одна транзакция.
- Контрольные суммы CRC32C на суперблоке, inode, блоках каталогов,
  блоках экстентов и блоках журнала. У данных файлов контрольных сумм нет.
- Проверка целостности (`VeloraFs::check`, fsck) читает всю файловую систему
  и сообщает о проблемах. Исправлять их она пока не умеет (NOT IMPLEMENTED).
- Ограничения версии 1: нет жёстких и символических ссылок, нет расширенных
  атрибутов, размер файла — до 2^32 блоков (16 ТиБ), не больше 265 экстентов
  на файл. Если для записи нужно больше экстентов, она завершается ошибкой
  `EFBIG`.

### Расположение на диске

Номера блоков. Все числа little-endian.

| Блоки | Содержимое |
|---|---|
| 0 | Суперблок |
| 1 … J | Журнал (J = 1/32 тома, от 32 до 1024 блоков) |
| далее | Битовая карта inode (бит на inode) |
| далее | Битовая карта блоков (бит на блок, 1 = занят) |
| далее | Таблица inode (16 inode по 256 байт в блоке) |
| `data_start` … | Данные файлов, блоки каталогов, блоки экстентов |

Число inode — 1 на 16 КиБ тома (от 64 до 2^20). Inode 0 не используется,
inode 1 — корневой каталог. Свободным считается inode, бит которого в карте
сброшен. Таблицу inode при форматировании не нужно обнулять.

### Суперблок (блок 0)

| Смещение | Поле |
|---|---|
| 0 | `VELORAFS` (8 байт) |
| 8 | версия формата (1) |
| 12 | размер блока (4096) |
| 16 … 127 | число блоков, число inode, свободные блоки, свободные inode, начало и длина журнала, начало и длина карт inode и блоков, начало и длина таблицы inode, `data_start`, `journal_seq` (по 8 байт) |
| 128 | UUID (16 байт) |
| 144 | метка (32 байта, UTF-8) |
| 176 | время создания |
| 184 | число монтирований (пока не увеличивается) |
| 4092 | CRC32C байтов 0 … 4091 |

### Inode (256 байт)

| Смещение | Поле |
|---|---|
| 0 | `mode`: тип (`0o040000` каталог, `0o100000` файл) и права |
| 4, 8 | uid, gid |
| 12 | число ссылок (файл — 1, каталог — 2 + подкаталоги) |
| 16 | размер в байтах |
| 24, 32, 40 | mtime, ctime, время создания (секунды с 1970 UTC) |
| 52 | число экстентов |
| 56 | родительский каталог (только для каталогов) |
| 64 | блок дополнительных экстентов (0 — нет) |
| 72 | флаги (пока 0) |
| 80 … 239 | 10 экстентов: логический блок (4), длина (4), физический блок (8) |
| 252 | CRC32C номера inode и байтов 0 … 251 |

Номер inode входит в контрольную сумму, поэтому inode, записанный не в свою
ячейку, обнаруживается.

Блок экстентов: `VEXT` (4), число экстентов (4), номер inode-владельца (8),
затем до 255 экстентов по 16 байт, CRC32C в конце блока.

### Каталоги

Каталог — файл из блоков записей. Запись: номер inode (8 байт, 0 — пусто),
длина записи (2), длина имени (1), тип (1: файл, 2: каталог), имя в UTF-8.
Записи выровнены по 8 байт и покрывают байты 0 … 4087 блока целиком;
в последних 4 байтах — CRC32C. Удаление присоединяет запись к предыдущей.
Записей `.` и `..` нет: VFS разбирает пути сама, а родитель хранится в inode.
Имена сравниваются побайтно (с учётом регистра), длина имени — до 255 байт.

### Журнал и порядок записи

Одна транзакция — одна операция (создание, запись, переименование и т. д.).
Изменённые блоки метаданных копятся в памяти. При фиксации:

1. Если операция писала данные файла, выполняется сброс кеша диска
   (данные попадают на диск раньше метаданных, которые на них указывают).
2. В журнал одной записью пишутся: блок-дескриптор (`VJDS`, номер
   последовательности, список домашних адресов), копии блоков, блок
   фиксации (`VJCM`, номер, CRC32C всех копий). Затем сброс кеша.
3. Блоки пишутся на свои места, сброс; суперблок пишется последним, сброс.

Суперблок входит в каждую транзакцию, и его копия несёт следующий номер
`journal_seq`. При монтировании транзакция в журнале воспроизводится, если
номер в дескрипторе равен `journal_seq` суперблока и все контрольные суммы
сходятся. После воспроизведения на диске уже новый суперблок, поэтому
повторно та же транзакция не применяется. Если сбой случился до записи
блока фиксации, транзакции нет, и на диске остаётся прежнее согласованное
состояние.

Освобождённые блоки снова выделяются только в следующих транзакциях,
поэтому данные, записанные до фиксации, никогда не попадают в блоки,
которые ещё принадлежат другому файлу на диске. Хвост последнего блока
файла обнуляется перед тем, как файл растёт, а не при усечении, чтобы
усечение не меняло данные до своей фиксации.

### Как проверено

- Юнит-тесты на хосте (`cargo test -p velorafs`): общий набор
  `velora_vfs::conformance`, сохранение после перемонтирования, файлы с
  дырами и блоком экстентов, каталог на много блоков, нехватка места,
  повреждённые суперблок, inode и каталог, монтирование только для чтения.
- Отключение питания после каждой записи на диск: сценарий из шести
  операций повторяется с обрывом после 1, 2, … N записей, с кешем записи
  диска и без него. После каждого обрыва том монтируется, fsck не находит
  ошибок, а файлы находятся в одном из допустимых состояний.
- В QEMU (тесты ядра): том создаётся на диске virtio-blk, проверяется,
  транзакция оставляется в журнале без записи на места; после второй
  загрузки ядро воспроизводит журнал при монтировании, и все файлы на месте.
  После каждой загрузки том проверяется на хосте `velora-fsck`.

## English

VeloraFS is Velora OS's own file system for the root of an installed system.
Code: `fs/velorafs` (crate `velorafs`, `no_std` + `alloc`). It builds into the
kernel and into host programs (`velora-fsck`) and is tested by host unit tests.

### Properties

- 4 KiB blocks; the device sector may be any divisor of 4096.
- Files are lists of extents (start, length): 10 in the inode, 255 more in one
  overflow block. Unwritten ranges (holes) read as zeros and use no space.
- Every inode has an owner, a group and POSIX permission bits (`rwx` for
  owner, group and others, plus setuid/setgid/sticky).
- Metadata journal; every operation is one transaction.
- CRC32C checksums on the superblock, inodes, directory blocks, extent blocks
  and journal blocks. File data has no checksum.
- The consistency check (`VeloraFs::check`, fsck) reads the whole file system
  and reports problems. Repair is NOT IMPLEMENTED yet.
- Version 1 limits: no hard or symbolic links, no extended attributes, files up
  to 2^32 blocks (16 TiB), at most 265 extents per file (a write that needs
  more fails with `EFBIG`).

### On-disk layout

Block numbers; all integers little-endian.

| Blocks | Contents |
|---|---|
| 0 | Superblock |
| 1 … J | Journal (J = 1/32 of the volume, 32 to 1024 blocks) |
| next | Inode bitmap (one bit per inode) |
| next | Block bitmap (one bit per block, 1 = used) |
| next | Inode table (16 inodes of 256 bytes per block) |
| `data_start` … | File data, directory blocks, extent blocks |

One inode per 16 KiB of volume (64 to 2^20). Inode 0 is unused, inode 1 is the
root directory. An inode is free when its bitmap bit is clear, so the inode
table does not need zeroing at format time.

The superblock, inode, extent block and directory record layouts are given in
the tables of the Russian section (offsets are the same). The inode CRC covers
the inode number too, so an inode written to the wrong slot is detected.
Directories are files made of 8-byte aligned records (inode number, record
length, name length, type, UTF-8 name) covering bytes 0…4087 of each block,
with a CRC32C in the last four bytes. There are no `.` and `..` records; the
parent is kept in the inode. Names compare byte by byte (case-sensitive), up
to 255 bytes.

### Journal and write order

One transaction per operation. Changed metadata blocks collect in memory. On
commit: (1) if the operation wrote file data, the disk cache is flushed, so
data reaches the disk before the metadata that points to it; (2) a descriptor
block (`VJDS`, sequence number, home addresses), the block copies and a commit
block (`VJCM`, sequence number, CRC32C of all copies) are written to the
journal in one request, then flushed; (3) the blocks are written home and
flushed, the superblock last, flushed again.

The superblock is part of every transaction and its copy carries the next
`journal_seq`. At mount a journal transaction is replayed when its sequence
number equals the superblock's `journal_seq` and every checksum matches; after
the replay the new superblock is on disk, so the same transaction is never
applied twice. A crash before the commit block is written leaves no
transaction and the previous consistent state on disk.

Freed blocks are reused only by later transactions, so data written before a
commit never lands in blocks another file still owns on disk. The tail of a
file's last block is zeroed before the file grows, not at truncation, so a
truncate never changes data before it commits.

### How it is tested

- Host unit tests (`cargo test -p velorafs`): the shared
  `velora_vfs::conformance` suite, persistence across remounts, sparse files
  with an extent block, directories over many blocks, running out of space,
  corrupted superblock, inode and directory, read-only mounts.
- A power cut after every disk write: a six-operation scenario is repeated with
  the power cut after 1, 2, … N writes, with and without a disk write cache.
  After each cut the volume mounts, fsck finds nothing, and the files are in
  one of the allowed states.
- In QEMU (kernel tests): a volume is created on a virtio-blk disk, checked,
  and a transaction is left in the journal without being written home; on the
  second boot the kernel replays it at mount and every file is present. After
  each boot the volume is checked on the host with `velora-fsck`.
