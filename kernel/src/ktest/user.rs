//! Stage 8 tests: pipes, the shell and the commands, ACPI power off data.
//! Interactive use (login, line editing, Tab, Ctrl+C) is checked by the
//! boot test in tests/qemu_run.py through the emulated keyboard.

use crate::fs::pipe::{Broken, CAPACITY, Pipe};
use crate::{acpi, exec, fs, kprint, sched};
use alloc::string::String;
use alloc::vec::Vec;
use velora_vfs::{Cred, Kind};

fn run(argv: &[&str]) -> i64 {
    let pid = exec::spawn(argv[0], argv, &["PATH=/bin", "HOME=/root"]).expect("spawn failed");
    sched::wait_pid(pid).expect("process vanished")
}

fn read(p: &str) -> String {
    let b =
        fs::with(|v| v.read_all(p, &Cred::ROOT)).unwrap_or_else(|e| panic!("read {}: {}", p, e));
    String::from_utf8(b).expect("not UTF-8")
}

fn write(p: &str, text: &str) {
    let now = fs::now();
    fs::with(|v| {
        let n = v.create(p, Kind::File, 0o644, &Cred::ROOT, now)?;
        v.write(n, 0, text.as_bytes(), now)
    })
    .unwrap();
}

pub fn pipe_buffer() {
    let p = Pipe::new();
    assert_eq!(p.write(b"abc"), Ok(3));
    assert_eq!(p.read(2), b"ab");
    assert_eq!(p.read(10), b"c");
    // Fill exactly to capacity without blocking.
    let big: Vec<u8> = (0..CAPACITY).map(|i| i as u8).collect();
    assert_eq!(p.write(&big), Ok(CAPACITY));
    assert_eq!(p.read(CAPACITY), big);
    p.close_end(true);
    assert!(
        p.read(10).is_empty(),
        "end of input after the writer closed"
    );
    let q = Pipe::new();
    q.close_end(false);
    assert_eq!(q.write(b"x"), Err(Broken));
}

pub fn acpi_power_off_data() {
    let fadt = acpi::get().fadt;
    assert!(fadt.pm1a_cnt != 0, "no PM1a control port");
    let dsdt = acpi::dsdt().expect("no DSDT");
    let s5 = velora_acpi::find_s5(dsdt);
    kprint!("(PM1a {:#x}, S5 {:?}) ", fadt.pm1a_cnt, s5);
    assert!(s5.is_some(), "no _S5 package in the DSDT");
}

pub fn shell_pipelines_and_variables() {
    let status = run(&[
        "/bin/vsh",
        "-c",
        "echo one two | cat > /tmp/k1; ls /bin | cat >> /tmp/k1; X=5; echo val=$X >> /tmp/k1; false_cmd_x || echo or-ok >> /tmp/k1",
    ]);
    assert_eq!(status, 0);
    let out = read("/tmp/k1");
    assert!(out.starts_with("one two\n"), "{:?}", out);
    assert!(
        out.contains("\nvsh\n") && out.contains("\nvelofetch\n"),
        "{:?}",
        out
    );
    assert!(
        out.contains("val=5\n") && out.ends_with("or-ok\n"),
        "{:?}",
        out
    );
    fs::with(|v| v.unlink("/tmp/k1", &Cred::ROOT, 0)).unwrap();
}

pub fn shell_script_and_exit_status() {
    write(
        "/tmp/script.vsh",
        "# a script\ncd /tmp\nmkdir sd\necho hi > sd/f\ncat < sd/f > out2 && echo ok >> out2\nrm -r sd\nexit 3\necho never > never\n",
    );
    assert_eq!(run(&["/bin/vsh", "/tmp/script.vsh"]), 3);
    assert_eq!(read("/tmp/out2"), "hi\nok\n");
    assert!(fs::with(|v| v.stat_path("/tmp/never", &Cred::ROOT)).is_err());
    assert!(fs::with(|v| v.stat_path("/tmp/sd", &Cred::ROOT)).is_err());
    assert_eq!(run(&["/bin/vsh", "-c", "nosuchcommand"]), 127);
    assert_eq!(run(&["/bin/vsh", "-c", "echo 'unclosed"]), 2);
    for p in ["/tmp/out2", "/tmp/script.vsh"] {
        fs::with(|v| v.unlink(p, &Cred::ROOT, 0)).unwrap();
    }
}

pub fn commands_work() {
    let status = run(&[
        "/bin/vsh",
        "-c",
        "cd /tmp && velofetch > vf; ps > ps; mem > mem; uptime > up; date > date; whoami > who; \
         cp /etc/motd m2 && mv m2 m3 && cat m3 > motd; rm m3; touch t; mkdir -p a/b/c; \
         cp -r a a2; ls -l /tmp > ls; ls a2/b > lsb; rm -r a a2 t; help > help",
    ]);
    assert_eq!(status, 0);
    let vf = read("/tmp/vf");
    assert!(
        vf.contains("OS:      Velora OS") && vf.contains("Memory:"),
        "{}",
        vf
    );
    assert!(read("/tmp/ps").contains("PID  PPID"));
    assert!(read("/tmp/mem").starts_with("total:"));
    assert!(read("/tmp/up").starts_with("up "));
    assert!(read("/tmp/date").ends_with(" UTC\n"));
    assert_eq!(read("/tmp/who"), "root\n");
    assert!(read("/tmp/motd").starts_with("Velora OS"));
    let ls = read("/tmp/ls");
    assert!(
        ls.contains(" root ") && ls.contains("vf\n") && ls.contains("drwxr-xr-x"),
        "{}",
        ls
    );
    assert_eq!(read("/tmp/lsb"), "c\n");
    assert!(read("/tmp/help").contains("velofetch"));
    for p in [
        "vf", "ps", "mem", "up", "date", "who", "motd", "ls", "lsb", "help",
    ] {
        fs::with(|v| v.unlink(&alloc::format!("/tmp/{}", p), &Cred::ROOT, 0)).unwrap();
    }
    assert!(fs::with(|v| v.stat_path("/tmp/a", &Cred::ROOT)).is_err());
}

pub fn passwords_are_hashed() {
    let shadow = read("/etc/shadow");
    for user in ["root", "user"] {
        let h = velora_auth::shadow_hash(&shadow, user).expect("no shadow entry");
        assert!(h.starts_with("$pbkdf2-sha256$"), "{}: {}", user, h);
        assert!(!h.contains("velora"), "plain password in /etc/shadow");
    }
    let st = fs::with(|v| v.stat_path("/etc/shadow", &Cred::ROOT)).unwrap();
    assert_eq!(st.mode, 0o600);
}
