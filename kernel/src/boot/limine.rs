//! Minimal bindings for the Limine boot protocol (base revision 6).
//!
//! Only the requests the kernel actually uses are declared. The layout
//! follows `limine.h` from the limine-protocol repository. The bootloader
//! scans the `.limine_requests` section between the start and end markers
//! and fills in the `response` pointers before jumping to `kmain`.

use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

/// Cleared once bootloader-reclaimable memory is handed to the allocator.
static RESPONSES_VALID: AtomicBool = AtomicBool::new(true);

/// Marks all responses as gone. Any later `response()` call panics.
pub fn invalidate_responses() {
    RESPONSES_VALID.store(false, Ordering::SeqCst);
}

const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];
const BASE_REVISION_MAGIC: [u64; 2] = [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc];

/// The base revision this kernel is written against.
pub const REQUESTED_BASE_REVISION: u64 = 6;

const fn request_id(a: u64, b: u64) -> [u64; 4] {
    [COMMON_MAGIC[0], COMMON_MAGIC[1], a, b]
}

#[repr(C, align(8))]
pub struct BaseRevision(UnsafeCell<[u64; 3]>);

// SAFETY: written only by the bootloader before the kernel runs; the kernel
// only reads it with volatile loads.
unsafe impl Sync for BaseRevision {}

impl BaseRevision {
    const fn new(revision: u64) -> Self {
        Self(UnsafeCell::new([
            BASE_REVISION_MAGIC[0],
            BASE_REVISION_MAGIC[1],
            revision,
        ]))
    }

    /// The bootloader sets the third word to 0 if it honours the revision.
    pub fn is_supported(&self) -> bool {
        // SAFETY: plain volatile read of a static.
        unsafe { ptr::read_volatile(&raw const (*self.0.get())[2]) == 0 }
    }

    /// Revision the bootloader actually used, if it reported one.
    pub fn loaded_revision(&self) -> Option<u64> {
        // SAFETY: plain volatile read of a static.
        let v = unsafe { ptr::read_volatile(&raw const (*self.0.get())[1]) };
        if v == BASE_REVISION_MAGIC[1] {
            None
        } else {
            Some(v)
        }
    }
}

#[repr(C, align(8))]
struct Marker<const N: usize>([u64; N]);

/// A Limine request with a response pointer the bootloader fills in.
#[repr(C, align(8))]
pub struct Request<R: 'static> {
    id: [u64; 4],
    revision: u64,
    response: UnsafeCell<*const R>,
}

// SAFETY: see `BaseRevision`.
unsafe impl<R> Sync for Request<R> {}

impl<R> Request<R> {
    const fn new(id: [u64; 4]) -> Self {
        Self {
            id,
            revision: 0,
            response: UnsafeCell::new(ptr::null()),
        }
    }

    pub fn response(&self) -> Option<&'static R> {
        assert!(
            RESPONSES_VALID.load(Ordering::SeqCst),
            "Limine response used after bootloader memory was released"
        );
        // SAFETY: the pointer is either null or points at bootloader-owned
        // memory that stays mapped until `invalidate_responses`.
        unsafe {
            let p = ptr::read_volatile(self.response.get());
            p.as_ref()
        }
    }
}

#[repr(C)]
pub struct BootloaderInfoResponse {
    pub revision: u64,
    name: *const u8,
    version: *const u8,
}

impl BootloaderInfoResponse {
    pub fn name(&self) -> &'static str {
        // SAFETY: Limine returns NUL-terminated ASCII strings.
        unsafe { c_str(self.name) }
    }
    pub fn version(&self) -> &'static str {
        // SAFETY: as above.
        unsafe { c_str(self.version) }
    }
}

#[repr(C)]
pub struct HhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

#[repr(C)]
pub struct RawFramebuffer {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    _unused: [u8; 7],
    pub edid_size: u64,
    pub edid: *const u8,
}

#[repr(C)]
pub struct FramebufferResponse {
    pub revision: u64,
    count: u64,
    framebuffers: *const *const RawFramebuffer,
}

impl FramebufferResponse {
    pub fn framebuffers(&self) -> &'static [&'static RawFramebuffer] {
        // SAFETY: Limine gives an array of `count` valid pointers.
        unsafe { slice_of_refs(self.framebuffers, self.count) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum MemoryKind {
    Usable = 0,
    Reserved = 1,
    AcpiReclaimable = 2,
    AcpiNvs = 3,
    BadMemory = 4,
    BootloaderReclaimable = 5,
    ExecutableAndModules = 6,
    Framebuffer = 7,
    ReservedMapped = 8,
}

impl MemoryKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Usable => "usable",
            Self::Reserved => "reserved",
            Self::AcpiReclaimable => "acpi-reclaimable",
            Self::AcpiNvs => "acpi-nvs",
            Self::BadMemory => "bad",
            Self::BootloaderReclaimable => "bootloader-reclaimable",
            Self::ExecutableAndModules => "kernel+modules",
            Self::Framebuffer => "framebuffer",
            Self::ReservedMapped => "reserved-mapped",
        }
    }
}

#[repr(C)]
pub struct MemmapEntry {
    pub base: u64,
    pub length: u64,
    kind: u64,
}

impl MemmapEntry {
    pub fn kind(&self) -> MemoryKind {
        match self.kind {
            0 => MemoryKind::Usable,
            2 => MemoryKind::AcpiReclaimable,
            3 => MemoryKind::AcpiNvs,
            4 => MemoryKind::BadMemory,
            5 => MemoryKind::BootloaderReclaimable,
            6 => MemoryKind::ExecutableAndModules,
            7 => MemoryKind::Framebuffer,
            8 => MemoryKind::ReservedMapped,
            // Unknown types are treated as reserved, which is always safe.
            _ => MemoryKind::Reserved,
        }
    }
}

#[repr(C)]
pub struct MemmapResponse {
    pub revision: u64,
    count: u64,
    entries: *const *const MemmapEntry,
}

impl MemmapResponse {
    pub fn entries(&self) -> &'static [&'static MemmapEntry] {
        // SAFETY: Limine gives an array of `count` valid pointers.
        unsafe { slice_of_refs(self.entries, self.count) }
    }
}

#[repr(C)]
pub struct CmdlineResponse {
    pub revision: u64,
    cmdline: *const u8,
}

impl CmdlineResponse {
    pub fn cmdline(&self) -> &'static str {
        // SAFETY: NUL-terminated string from the bootloader.
        unsafe { c_str(self.cmdline) }
    }
}

#[repr(C)]
pub struct ExecutableAddressResponse {
    pub revision: u64,
    pub physical_base: u64,
    pub virtual_base: u64,
}

#[repr(C)]
pub struct DateAtBootResponse {
    pub revision: u64,
    /// UNIX timestamp in seconds.
    pub timestamp: i64,
}

/// A file loaded by the bootloader (`limine_file`).
#[repr(C)]
pub struct File {
    pub revision: u64,
    pub address: *const u8,
    pub size: u64,
    path: *const u8,
    string: *const u8,
    /// `MEDIA_GENERIC` (a disk), `MEDIA_OPTICAL` or `MEDIA_TFTP`.
    pub media_type: u32,
    unused: u32,
    tftp_ip: u32,
    tftp_port: u32,
    /// 1-based partition number, 0 for a whole disk without partitions.
    pub partition_index: u32,
    mbr_disk_id: u32,
    gpt_disk_uuid: [u8; 16],
    /// The GPT unique partition GUID as stored on the disk (zero if none).
    pub gpt_part_uuid: [u8; 16],
    part_uuid: [u8; 16],
}

pub const MEDIA_GENERIC: u32 = 0;

impl File {
    pub fn path(&self) -> &'static str {
        // SAFETY: NUL-terminated string from the bootloader.
        unsafe { c_str(self.path) }
    }
}

#[repr(C)]
pub struct ExecutableFileResponse {
    pub revision: u64,
    file: *const File,
}

impl ExecutableFileResponse {
    pub fn file(&self) -> Option<&'static File> {
        // SAFETY: Limine gives a valid pointer (or null).
        unsafe { self.file.as_ref() }
    }
}

#[repr(C)]
pub struct ModuleResponse {
    pub revision: u64,
    count: u64,
    modules: *const *const File,
}

impl ModuleResponse {
    pub fn modules(&self) -> &'static [&'static File] {
        // SAFETY: Limine gives an array of `count` valid pointers.
        unsafe { slice_of_refs(self.modules, self.count) }
    }
}

#[repr(C)]
pub struct RsdpResponse {
    pub revision: u64,
    pub address: *const u8,
}

/// The multiprocessor request has a flags word after the response
/// pointer (bit 0 would ask for x2APIC; the kernel keeps the firmware's
/// APIC mode and passes 0).
#[repr(C, align(8))]
pub struct MpRequest {
    id: [u64; 4],
    revision: u64,
    response: UnsafeCell<*const MpResponse>,
    flags: u64,
}

// SAFETY: see `BaseRevision`.
unsafe impl Sync for MpRequest {}

impl MpRequest {
    const fn new() -> Self {
        Self {
            id: request_id(0x95a67b819a1b857e, 0xa0b61b723b6a73e0),
            revision: 0,
            response: UnsafeCell::new(ptr::null()),
            flags: 0,
        }
    }

    pub fn response(&self) -> Option<&'static MpResponse> {
        assert!(
            RESPONSES_VALID.load(Ordering::SeqCst),
            "Limine response used after bootloader memory was released"
        );
        // SAFETY: as in `Request::response`.
        unsafe { ptr::read_volatile(self.response.get()).as_ref() }
    }
}

#[repr(C)]
pub struct MpResponse {
    pub revision: u64,
    pub flags: u32,
    pub bsp_lapic_id: u32,
    count: u64,
    cpus: *const *const MpInfo,
}

impl MpResponse {
    pub fn cpus(&self) -> &'static [&'static MpInfo] {
        // SAFETY: Limine gives an array of `count` valid pointers.
        unsafe { slice_of_refs(self.cpus, self.count) }
    }
}

/// One processor. An application processor waits in the bootloader
/// until `goto_address` is written, then jumps there with RDI pointing at
/// this structure, on a small stack in bootloader-reclaimable memory.
#[repr(C)]
pub struct MpInfo {
    pub processor_id: u32,
    pub lapic_id: u32,
    _reserved: u64,
    goto_address: core::sync::atomic::AtomicU64,
    pub extra_argument: core::sync::atomic::AtomicU64,
}

impl MpInfo {
    /// Starts this processor at `entry` with `arg` in `extra_argument`.
    pub fn start(&self, entry: unsafe extern "C" fn(*const MpInfo) -> !, arg: u64) {
        self.extra_argument.store(arg, Ordering::SeqCst);
        self.goto_address
            .store(entry as usize as u64, Ordering::SeqCst);
    }
}

unsafe fn c_str(p: *const u8) -> &'static str {
    if p.is_null() {
        return "";
    }
    let mut len = 0;
    // SAFETY: caller guarantees a NUL-terminated string.
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        core::str::from_utf8(core::slice::from_raw_parts(p, len)).unwrap_or("<invalid utf-8>")
    }
}

unsafe fn slice_of_refs<T>(p: *const *const T, count: u64) -> &'static [&'static T] {
    if p.is_null() || count == 0 {
        return &[];
    }
    // SAFETY: `*const T` and `&T` have the same layout; Limine guarantees
    // every pointer in the array is non-null and valid.
    unsafe { core::slice::from_raw_parts(p.cast::<&'static T>(), count as usize) }
}

// ---------------------------------------------------------------------------
// Requests. The order inside the section does not matter.

#[used]
#[unsafe(link_section = ".limine_requests_start")]
static START_MARKER: Marker<4> = Marker([
    0xf6b8f4b39de7d1ae,
    0xfab91a6940fcb9cf,
    0x785c6ed015d3e316,
    0x181e920a7852b9d9,
]);

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static BASE_REVISION: BaseRevision = BaseRevision::new(REQUESTED_BASE_REVISION);

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static BOOTLOADER_INFO: Request<BootloaderInfoResponse> =
    Request::new(request_id(0xf55038d8e2a1202f, 0x279426fcf5f59740));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static HHDM: Request<HhdmResponse> =
    Request::new(request_id(0x48dcf1cb8ad2b852, 0x63984e959a98244b));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static FRAMEBUFFER: Request<FramebufferResponse> =
    Request::new(request_id(0x9d5827dcd881dd75, 0xa3148604f6fab11b));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static MEMMAP: Request<MemmapResponse> =
    Request::new(request_id(0x67cf3d9d378a806f, 0xe304acdfc50c3c62));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static CMDLINE: Request<CmdlineResponse> =
    Request::new(request_id(0x4b161536e598651e, 0xb390ad4a2f1f303a));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static EXECUTABLE_ADDRESS: Request<ExecutableAddressResponse> =
    Request::new(request_id(0x71ba76863cc55f63, 0xb2644a48c516a487));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static DATE_AT_BOOT: Request<DateAtBootResponse> =
    Request::new(request_id(0x502746e184c088aa, 0xfbc5ec83e6327893));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static RSDP: Request<RsdpResponse> =
    Request::new(request_id(0xc5e77b6b397e7b43, 0x27637845accdcf3c));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static MODULES: Request<ModuleResponse> =
    Request::new(request_id(0x3e7e279702be32af, 0xca1c4f3bd1280cee));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static EXECUTABLE_FILE: Request<ExecutableFileResponse> =
    Request::new(request_id(0xad97e90e83f1ed67, 0x31eb5d1c5ff23b69));

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static MP: MpRequest = MpRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static END_MARKER: Marker<2> = Marker([0xadc0e0531bb10d03, 0x9572709f31764c62]);
