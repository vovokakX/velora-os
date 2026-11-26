/* Linker script for Velora user programs (static ELF64, ET_EXEC).
 * Every output section starts on its own page so each page has a single
 * set of permissions (W^X): .text R-X, .rodata R--, .data/.bss RW-. */
OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

SECTIONS
{
    . = 0x400000;

    .text : ALIGN(4096) {
        *(.text .text.*)
    }

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4096) {
        *(.data.rel.ro .data.rel.ro.*)
        *(.got .got.*)
        *(.data .data.*)
    }

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.comment)
    }
}
