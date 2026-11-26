//! Stage 5 tests: initramfs, ELF loading, PE detection, user programs.

use crate::exec::{self, ExecError};
use crate::mm::frame;
use crate::mm::paging::AddressSpace;
use crate::{initramfs, kprint, sched};

pub fn initramfs_has_programs() {
    let mut names = alloc::vec::Vec::new();
    initramfs::for_each_file(|p, _, mode| names.push((alloc::string::String::from(p), mode)));
    kprint!("({} files) ", names.len());
    for want in ["bin/hello", "bin/vtest", "etc/motd"] {
        assert!(names.iter().any(|(n, _)| n == want), "{} missing", want);
    }
    let (_, mode) = names.iter().find(|(n, _)| n == "bin/hello").unwrap();
    assert_eq!(mode & 0o111, 0o111, "programs must be executable");
    assert!(velora_exe::elf::probe(
        initramfs::read("/bin/hello").unwrap()
    ));
    assert!(initramfs::read("/bin/nothing").is_none());
}

fn run(path: &str, argv: &[&str], envp: &[&str]) -> i64 {
    let pid = exec::spawn(path, argv, envp).expect("spawn failed");
    sched::wait_pid(pid).expect("process vanished")
}

pub fn elf_program_runs() {
    assert_eq!(run("/bin/hello", &["/bin/hello", "ktest"], &["A=1"]), 0);
}

/// The user-space test suite: vlibc, heap, mmap, spawn/wait, faults.
pub fn user_space_test_suite() {
    crate::kprintln!();
    let status = run("/bin/vtest", &["/bin/vtest", "all"], &["VELORA_TEST=yes"]);
    assert_eq!(status, 0, "vtest reported {} failures", status);
}

pub fn missing_and_non_executable_files() {
    assert_eq!(exec::spawn("/bin/none", &[], &[]), Err(ExecError::NotFound));
    assert_eq!(
        exec::spawn("/etc/motd", &[], &[]),
        Err(ExecError::NotExecutable)
    );
}

pub fn pe_files_are_recognised_but_not_run() {
    let a = AddressSpace::new().unwrap();
    let image = velora_exe::pe::sample_headers();
    match exec::load_image(&image, &a) {
        Err(ExecError::Unsupported(msg)) => assert!(msg.contains("Win32"), "message: {}", msg),
        Err(e) => panic!("unexpected error {:?}", e),
        Ok(_) => panic!("a PE file was loaded"),
    }
    assert_eq!(
        a.user_pages(),
        0,
        "nothing may be mapped for a refused file"
    );
}

pub fn broken_elf_is_rejected() {
    let a = AddressSpace::new().unwrap();
    let mut image = alloc::vec::Vec::from(initramfs::read("/bin/hello").unwrap());
    image[18] = 0xb7; // e_machine = AArch64
    assert!(matches!(
        exec::load_image(&image, &a),
        Err(ExecError::Invalid(_))
    ));
    image[18] = 62;
    image[16] = 3; // ET_DYN
    assert!(matches!(
        exec::load_image(&image, &a),
        Err(ExecError::Invalid(_))
    ));
    assert_eq!(a.user_pages(), 0);
}

pub fn too_many_arguments_are_refused() {
    let big = alloc::string::String::from_utf8(alloc::vec![b'x'; 70 * 1024]).unwrap();
    assert_eq!(
        exec::spawn("/bin/hello", &[&big], &[]),
        Err(ExecError::TooManyArgs)
    );
}

pub fn programs_do_not_leak_memory() {
    run("/bin/hello", &["/bin/hello"], &[]);
    let before = frame::stats().free_frames;
    for _ in 0..5 {
        assert_eq!(run("/bin/hello", &["/bin/hello"], &[]), 0);
    }
    let after = frame::stats().free_frames;
    assert!(
        before - after.min(before) <= 4,
        "{} frames leaked",
        before - after
    );
    assert!(sched::processes().is_empty(), "processes left behind");
}
