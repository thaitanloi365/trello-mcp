//! Finding, measuring and stopping the running trello-mcp servers with libproc, as the tray's
//! Procs.swift does; the rules they are read by live in servers.rs.

use std::{
    collections::{HashMap, HashSet},
    ffi::{CStr, c_int},
    mem::{MaybeUninit, size_of},
    sync::OnceLock,
    time::Duration,
};

use super::servers::{Sample, ServerId, is_server, parse_proc_args};

/// The server binary's process name; the tray is trello-mcp-tray, so it never matches.
pub(crate) const SERVER_NAME: &str = "trello-mcp";
/// Between SIGTERM and SIGKILL.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(5);
/// libproc.h; the libc crate does not export it.
const PROC_UID_ONLY: u32 = 4;

/// CPU times come in Mach ticks, not ns, on Apple silicon.
#[allow(deprecated)] // libc points to mach2; one call is not worth a dependency
fn timebase() -> (u64, u64) {
    static T: OnceLock<(u64, u64)> = OnceLock::new();
    *T.get_or_init(|| {
        let mut t = libc::mach_timebase_info { numer: 0, denom: 0 };
        let rc = unsafe { libc::mach_timebase_info(&mut t) };
        if rc == 0 && t.denom != 0 {
            (u64::from(t.numer), u64::from(t.denom))
        } else {
            (1, 1)
        }
    })
}

/// The size of the argument area, for KERN_PROCARGS2.
fn arg_max() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        let (mut n, mut size): (c_int, usize) = (0, size_of::<c_int>());
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                (&raw mut n).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 && n > 0 {
            n as usize
        } else {
            1 << 20
        }
    })
}

/// This user's pids: the only processes the dashboard may stop.
fn user_pids() -> Vec<i32> {
    let uid = unsafe { libc::getuid() };
    let bytes = unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, std::ptr::null_mut(), 0) };
    if bytes <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0i32; bytes as usize / size_of::<i32>() + 64]; // room for processes started since
    let got = unsafe {
        libc::proc_listpids(
            PROC_UID_ONLY,
            uid,
            pids.as_mut_ptr().cast(),
            (pids.len() * size_of::<i32>()) as c_int,
        )
    };
    pids.truncate(got.max(0) as usize / size_of::<i32>());
    pids.retain(|p| *p > 0);
    pids
}

fn proc_name(pid: i32) -> Option<String> {
    let mut b = [0u8; 64]; // proc_name fills up to 2 * MAXCOMLEN
    let n = unsafe { libc::proc_name(pid, b.as_mut_ptr().cast(), b.len() as u32) };
    if n <= 0 {
        return None;
    }
    CStr::from_bytes_until_nul(&b)
        .ok()
        .map(|c| c.to_string_lossy().into_owned())
}

/// Parent pid and start time (µs); None once the process is gone.
fn bsd_info(pid: i32) -> Option<(i32, u64)> {
    let mut i = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = size_of::<libc::proc_bsdinfo>() as c_int;
    if unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, i.as_mut_ptr().cast(), size) }
        != size
    {
        return None;
    }
    let i = unsafe { i.assume_init() };
    Some((
        i.pbi_ppid as i32,
        i.pbi_start_tvsec * 1_000_000 + i.pbi_start_tvusec,
    ))
}

/// The working folder.
fn folder(pid: i32) -> Option<String> {
    let mut v = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = size_of::<libc::proc_vnodepathinfo>() as c_int;
    if unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            v.as_mut_ptr().cast(),
            size,
        )
    } != size
    {
        return None;
    }
    let v = unsafe { v.assume_init() };
    // vip_path is MAXPATHLEN (1024) bytes, typed as [[c_char; 32]; 32] by the libc crate.
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(v.pvi_cdir.vip_path.as_ptr().cast(), 1024) };
    CStr::from_bytes_until_nul(bytes)
        .ok()
        .map(|c| c.to_string_lossy().into_owned())
}

/// The binary.
fn path(pid: i32) -> Option<String> {
    let mut b = vec![0u8; 4096]; // PROC_PIDPATHINFO_MAXSIZE
    let n = unsafe { libc::proc_pidpath(pid, b.as_mut_ptr().cast(), b.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&b[..n as usize]).into_owned())
}

fn args(pid: i32) -> Option<Vec<String>> {
    let mut size = arg_max();
    let mut b = vec![0u8; size];
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            b.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    parse_proc_args(&b[..size])
}

/// CPU time in ns and memory footprint in bytes, as Activity Monitor counts them; None once it is gone.
fn usage(pid: i32) -> Option<(u64, u64)> {
    let mut ri = MaybeUninit::<libc::rusage_info_v2>::zeroed();
    if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, ri.as_mut_ptr().cast()) } != 0 {
        return None;
    }
    let ri = unsafe { ri.assume_init() };
    let (numer, denom) = timebase();
    Some((
        (ri.ri_user_time + ri.ri_system_time) * numer / denom,
        ri.ri_phys_footprint,
    ))
}

/// What never changes for a server: read once per (pid, start time).
#[derive(Clone)]
struct Fixed {
    path: String,
    ppid: i32,
    parent_started: Option<u64>,
    folder: Option<String>,
}

/// Finds and measures the running servers; a repeat scan reads only names, parents and usage.
#[derive(Default)]
pub(crate) struct Scanner {
    fixed: HashMap<ServerId, Fixed>,
    others: HashSet<ServerId>, // trello-mcp processes that are not servers
}

impl Scanner {
    pub(crate) fn scan(&mut self) -> Vec<Sample> {
        let (mut out, mut next_fixed, mut next_others) =
            (Vec::new(), HashMap::new(), HashSet::new());
        for pid in user_pids() {
            if proc_name(pid).as_deref() != Some(SERVER_NAME) {
                continue;
            }
            // A failing call means the process just exited: skip it.
            let Some((ppid, start)) = bsd_info(pid) else {
                continue;
            };
            let id = ServerId { pid, start };
            if self.others.contains(&id) {
                next_others.insert(id);
                continue;
            }
            let f = match self.fixed.get(&id) {
                Some(f) => f.clone(),
                None => {
                    let Some(argv) = args(pid) else { continue };
                    if !is_server(&argv) {
                        next_others.insert(id);
                        continue;
                    }
                    // The session is the parent; launchd (1) or a parent that is gone means none.
                    let session = if ppid > 1 { bsd_info(ppid) } else { None };
                    Fixed {
                        path: path(pid)
                            .or_else(|| argv.first().cloned())
                            .unwrap_or_else(|| SERVER_NAME.into()),
                        ppid,
                        parent_started: session.map(|(_, s)| s),
                        folder: session.and_then(|_| folder(ppid)),
                    }
                }
            };
            let Some((cpu_ns, mem)) = usage(pid) else {
                continue;
            };
            // A server whose parent exited is adopted by launchd: it has no session from then on.
            let session = ppid == f.ppid;
            out.push(Sample {
                id,
                ppid,
                parent_started: if session { f.parent_started } else { None },
                folder: if session { f.folder.clone() } else { None },
                path: f.path.clone(),
                cpu_ns,
                mem,
            });
            next_fixed.insert(id, f);
        }
        self.fixed = next_fixed;
        self.others = next_others;
        out
    }
}

/// The process still runs under this pid and start time, with this name.
fn alive(id: ServerId, name: &str) -> bool {
    proc_name(id.pid).as_deref() == Some(name) && bsd_info(id.pid).map(|(_, s)| s) == Some(id.start)
}

/// Why kill failed; None when the process was already gone.
fn stop_failure(pid: i32, err: i32) -> Option<String> {
    let text = unsafe { CStr::from_ptr(libc::strerror(err)) }.to_string_lossy();
    (err != libc::ESRCH).then(|| format!("Could not stop PID {pid}: {text}"))
}

/// SIGTERM if `id` still runs as `name`. Ok(false): gone, or its pid now belongs to another
/// process, so nothing was sent. The caller follows up with `kill_if_alive` after STOP_GRACE.
pub(crate) fn terminate(id: ServerId, name: &str) -> Result<bool, String> {
    if !alive(id, name) {
        return Ok(false);
    }
    if unsafe { libc::kill(id.pid, libc::SIGTERM) } != 0 {
        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return stop_failure(id.pid, err).map_or(Ok(false), Err);
    }
    Ok(true)
}

/// SIGKILL if `id` still runs as `name`.
pub(crate) fn kill_if_alive(id: ServerId, name: &str) {
    if alive(id, name) {
        unsafe { libc::kill(id.pid, libc::SIGKILL) };
    }
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::process::ExitStatusExt,
        path::Path,
        process::{Child, Command, Stdio},
        thread::sleep,
        time::Duration,
    };

    use super::*;

    const FAKE: &str = "dashboard::procs::tests::fake_server_sleeps";

    /// The fake server: this test binary, copied as trello-mcp, runs only this test, which sleeps.
    /// Copies of system binaries (arm64e) are killed on launch, so the tray's selftest does the same.
    #[test]
    #[ignore = "helper: the fake server the scan tests start"]
    fn fake_server_sleeps() {
        if std::env::var_os("TRELLO_MCP_FAKE_SERVER").is_some() {
            sleep(Duration::from_secs(30));
        }
    }

    /// This test binary copied into `dir` as trello-mcp, started with `first` as its first argument.
    fn fake(dir: &Path, first: &str) -> Child {
        let exe = dir.join(SERVER_NAME);
        if !exe.exists() {
            std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        }
        Command::new(&exe)
            .args([first, "--exact", FAKE, "--ignored"])
            .env("TRELLO_MCP_FAKE_SERVER", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn wait_for(mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            sleep(Duration::from_millis(10));
        }
    }

    fn cpu_now() -> u64 {
        let mut t = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut t) };
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    }

    #[test]
    fn own_process() {
        let me = std::process::id() as i32;
        let (ppid, start) = bsd_info(me).expect("own bsd info");
        assert!(ppid == unsafe { libc::getppid() } && start > 0);
        assert_eq!(
            proc_name(me)
                .as_deref()
                .map(|n| n.starts_with("trello_mcp")),
            Some(true),
            "{:?}",
            proc_name(me)
        );
        assert_eq!(args(me), Some(std::env::args().collect::<Vec<_>>()));
        assert!(user_pids().contains(&me));
        assert_eq!(
            folder(me).map(std::path::PathBuf::from),
            std::env::current_dir().ok()
        );
        assert!(usage(me).expect("own usage").1 > 1 << 20);
        assert!(usage(-1).is_none() && bsd_info(-1).is_none() && proc_name(-1).is_none());
    }

    #[test]
    fn cpu_time_is_in_nanoseconds() {
        // Mach ticks, not ns, on Apple silicon: 0.3 s of CPU must read as about 0.3 s.
        let me = std::process::id() as i32;
        let u0 = usage(me).unwrap().0;
        let c0 = cpu_now();
        let mut n = 0u64;
        while cpu_now() - c0 < 300_000_000 {
            n = std::hint::black_box(n.wrapping_add(1));
        }
        let burnt = (usage(me).unwrap().0 - u0) as f64 / 1e9;
        assert!(
            (0.15..0.9).contains(&burnt),
            "{burnt} s of CPU for 0.3 s busy"
        );
    }

    #[test]
    fn scan_finds_servers_not_dashboards_and_stop_checks_identity() {
        let tmp = tempfile::Builder::new()
            .prefix("procs test ")
            .tempdir()
            .unwrap();
        let mut server = fake(tmp.path(), "serve");
        let mut dash = fake(tmp.path(), "dashboard");
        let (sp, dp) = (server.id() as i32, dash.id() as i32);
        let mut sc = Scanner::default();
        let mut found = None;
        wait_for(|| {
            found = sc.scan().into_iter().find(|s| s.id.pid == sp);
            found.is_some()
        });
        let found = found.expect("the fake server is found");
        let dir_name = tmp
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            found.path.ends_with(&format!("{dir_name}/trello-mcp")),
            "binary path with a space: {found:?}"
        );
        assert_eq!(
            found.ppid,
            std::process::id() as i32,
            "its session is this test process"
        );
        assert_eq!(
            found.parent_started,
            bsd_info(std::process::id() as i32).map(|(_, s)| s)
        );
        assert_eq!(
            found.folder.as_deref().map(Path::new),
            std::env::current_dir().ok().as_deref()
        );
        assert!(found.mem > 0);
        wait_for(|| args(dp).is_some_and(|a| a.get(1).map(String::as_str) == Some("dashboard")));
        assert!(
            sc.scan().iter().all(|s| s.id.pid != dp),
            "a dashboard is not a server"
        );

        // A reused pid (another start time) or another name is never signalled.
        assert_eq!(
            terminate(
                ServerId {
                    pid: sp,
                    start: found.id.start + 1
                },
                SERVER_NAME
            ),
            Ok(false)
        );
        assert_eq!(terminate(found.id, "sleep"), Ok(false));
        assert!(server.try_wait().unwrap().is_none());
        assert_eq!(terminate(found.id, SERVER_NAME), Ok(true));
        assert_eq!(server.wait().unwrap().signal(), Some(libc::SIGTERM));
        assert!(
            sc.scan().iter().all(|s| s.id != found.id),
            "a stopped server leaves the scan"
        );
        dash.kill().unwrap();
        dash.wait().unwrap();
    }

    #[test]
    fn sigkill_after_the_grace_period() {
        let mut stubborn = Command::new("/bin/sh")
            .args(["-c", "trap '' TERM; exec /bin/sleep 30"])
            .spawn()
            .unwrap();
        let pid = stubborn.id() as i32;
        wait_for(|| proc_name(pid).as_deref() == Some("sleep"));
        let id = ServerId {
            pid,
            start: bsd_info(pid).unwrap().1,
        };
        assert_eq!(terminate(id, "sleep"), Ok(true));
        sleep(Duration::from_millis(300));
        assert!(stubborn.try_wait().unwrap().is_none(), "it ignores SIGTERM");
        kill_if_alive(id, "sleep");
        assert_eq!(stubborn.wait().unwrap().signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn stop_failure_text() {
        assert_eq!(
            stop_failure(4648, libc::EPERM).as_deref(),
            Some("Could not stop PID 4648: Operation not permitted")
        );
        assert_eq!(stop_failure(4648, libc::ESRCH), None);
    }
}
