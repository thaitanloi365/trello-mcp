// Finding and measuring the running trello-mcp servers with libproc, stopping one, and the pure
// rules and text the panel shows them by (grouping, activity, labels), as in gws-mcp's tray. No AppKit here; main.swift's SELFTEST block checks it all.
import Foundation

/// The server binary's process name. The tray itself is trello-mcp-tray, so it never matches.
let serverName = "trello-mcp"
let userHome = FileManager.default.homeDirectoryForCurrentUser.path

/// A process's identity: the start time tells a reused pid apart.
struct ServerID: Hashable {
    let pid: pid_t
    let start: UInt64   // µs since 1970
}

/// One server as a scan sees it.
struct ServerSample: Equatable {
    let id: ServerID
    let ppid: pid_t
    let parentStarted: UInt64?  // the session's start time (µs); nil: no session (parent is launchd or gone)
    let folder: String?         // the session's working folder
    let path: String            // the binary
    let cpuNs: UInt64           // CPU time so far
    let mem: UInt64             // physical footprint in bytes: Activity Monitor's Memory
}

/// What the tray knows about one server across scans.
struct ServerRow: Equatable {
    var s: ServerSample
    var cpu: Double?        // percent of one core since the previous scan; nil on the first
    var lastActive: Date?   // the last scan that saw it work; nil: not seen since the tray started

    var id: ServerID { s.id }
}

// MARK: - Arguments

/// Arguments after the binary, with --config and its value taken out.
func plainArgs(_ argv: [String]) -> [String] {
    var out: [String] = [], it = argv.dropFirst().makeIterator()
    while let a = it.next() {
        if a == "--config" { _ = it.next() } else if !a.hasPrefix("--config=") { out.append(a) }
    }
    return out
}

/// trello-mcp serves when its first argument, --config aside, is `serve` or there is none.
func isServer(_ argv: [String]) -> Bool { plainArgs(argv).first.map { $0 == "serve" } ?? true }

/// argv from a KERN_PROCARGS2 buffer: argc (Int32), the exec path, NUL padding, then argc NUL-ended strings.
func parseProcArgs(_ b: [UInt8]) -> [String]? {
    guard b.count >= 4 else { return nil }
    let argc = Int(b.withUnsafeBytes { $0.loadUnaligned(as: Int32.self) })
    var i = 4
    while i < b.count, b[i] != 0 { i += 1 }     // exec path
    while i < b.count, b[i] == 0 { i += 1 }     // padding
    var out: [String] = []
    while out.count < argc, i < b.count {
        let s = i
        while i < b.count, b[i] != 0 { i += 1 }
        guard i < b.count else { return nil }   // cut off before its NUL
        out.append(String(decoding: b[s..<i], as: UTF8.self))
        i += 1
    }
    return out.count == argc ? out : nil
}

// MARK: - Servers

/// CPU time a server must burn between two scans to count as working: one tool call
/// costs a few ms; an idle server adds well under 1 ms per 2 s.
let activeCPU: UInt64 = 2_000_000

/// The tray reads the servers' CPU times this often even while the panel is closed, so ACTIVE
/// knows when each server last worked. One scan costs about 2 ms.
let minuteScan: TimeInterval = 60

/// The previous scan as the baseline for this one; nil when it is more than three minute-scans old
/// (the Mac slept), so work done in that gap gets no made-up time.
func baseline(_ prev: Date?, at: Date) -> Date? {
    guard let prev, at.timeIntervalSince(prev) <= 3 * minuteScan else { return nil }
    return prev
}

/// Folds a scan into the rows: history carries over by ServerID, gone servers drop out. Without a
/// baseline (the first scan, or after a long gap) there is no CPU figure; the last activity is kept
/// unless the server worked in the gap, when its time is unknown.
func advance(_ old: [ServerRow], _ scan: [ServerSample], prevAt: Date?, at: Date) -> [ServerRow] {
    let byID = Dictionary(old.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
    return scan.map { s in
        guard let o = byID[s.id] else { return ServerRow(s: s, cpu: nil, lastActive: nil) }
        guard let prevAt, at > prevAt else {
            return ServerRow(s: s, cpu: nil, lastActive: s.cpuNs < o.s.cpuNs + activeCPU ? o.lastActive : nil)
        }
        let d = s.cpuNs > o.s.cpuNs ? s.cpuNs - o.s.cpuNs : 0
        return ServerRow(s: s, cpu: Double(d) / 1e9 / at.timeIntervalSince(prevAt) * 100, lastActive: d >= activeCPU ? at : o.lastActive)
    }
}

/// Whether two scans hold the same servers in the same groups; then a tick only updates numbers.
func sameShape(_ a: [ServerRow], _ b: [ServerRow]) -> Bool {
    let key = { (r: ServerRow) in "\(r.id.pid) \(r.id.start) \(r.s.folder ?? "")" }
    return Set(a.map(key)) == Set(b.map(key))
}

/// "now" under a minute, then "12m ago", "3h ago", "2d ago"; "—" before any activity was seen.
func activeText(_ last: Date?, now: Date) -> String {
    guard let last else { return "—" }
    let m = max(0, Int(now.timeIntervalSince(last) / 60))
    return m < 1 ? "now" : m < 60 ? "\(m)m ago" : m < 1440 ? "\(m / 60)h ago" : "\(m / 1440)d ago"
}

/// The Build cell: "installed" for the tray's CLI, the worktree's name under
/// .claude/worktrees, else the last two folders of the binary's path.
func buildLabel(_ path: String, cli: String) -> String {
    if path == cli { return "installed" }
    if let r = path.range(of: "/.claude/worktrees/") { return String(path[r.upperBound...].prefix { $0 != "/" }) }
    let dirs = path.split(separator: "/").dropLast()
    return dirs.isEmpty ? path : dirs.suffix(2).joined(separator: "/")
}

let noSession = "No session"

/// A group's label: the last two folders of its session's working folder, "~" for home.
func groupLabel(_ folder: String?, home: String) -> String {
    guard let folder, !folder.isEmpty else { return noSession }
    if folder == home { return "~" }
    let parts = folder.split(separator: "/")
    return parts.isEmpty ? folder : parts.suffix(2).joined(separator: "/")
}

struct ServerGroup: Equatable {
    let label: String
    let rows: [ServerRow]
}

/// Groups by session folder, alphabetically with "No session" last; rows oldest session first.
func groups(_ rows: [ServerRow], home: String) -> [ServerGroup] {
    Dictionary(grouping: rows) { groupLabel($0.s.folder, home: home) }
        .map { ServerGroup(label: $0.key, rows: $0.value.sorted {
            ($0.s.parentStarted ?? $0.id.start, $0.id.pid) < ($1.s.parentStarted ?? $1.id.start, $1.id.pid)
        }) }
        .sorted { ($0.label == noSession ? 1 : 0, $0.label.lowercased()) < ($1.label == noSession ? 1 : 0, $1.label.lowercased()) }
}

/// Distinct sessions among the rows.
func sessionCount(_ rows: [ServerRow]) -> Int {
    Set(rows.compactMap { r in r.s.parentStarted.map { "\(r.s.ppid) \($0)" } }).count
}

/// A group row's cells: the count after its label ("2" folded; "2 · 1 session" open), then its
/// latest ACTIVE, total CPU and total memory, under the table's columns.
func groupCells(_ g: ServerGroup, open: Bool, now: Date) -> [String] {
    let n = sessionCount(g.rows)
    let count = open && n > 0 ? "\(g.rows.count) · \(n) session\(n == 1 ? "" : "s")" : "\(g.rows.count)"
    return [count, activeText(g.rows.compactMap(\.lastActive).max(), now: now), cpuText(totalCPU(g.rows)), memText(g.rows.reduce(0) { $0 + $1.s.mem })]
}

/// Total CPU of the rows; nil while every row still waits for its second scan.
func totalCPU(_ rows: [ServerRow]) -> Double? {
    let known = rows.compactMap(\.cpu)
    return known.isEmpty ? nil : known.reduce(0, +)
}

/// "6d 21h", "5h 12m" or "12m", rounded down to the minute.
func countdown(_ t: TimeInterval) -> String {
    let m = max(0, Int(t / 60))
    return m >= 1440 ? "\(m / 1440)d \(m % 1440 / 60)h" : m >= 60 ? "\(m / 60)h \(m % 60)m" : "\(m)m"
}

/// The Up cell: how long the owning session has run ("2d 9h", "45m"); "—" without one.
func upText(_ r: ServerRow, now: Date) -> String {
    r.s.parentStarted.map { countdown(now.timeIntervalSince1970 - Double($0) / 1e6) } ?? "—"
}

/// The section's figures: servers, sessions, memory, CPU.
func serverFigures(_ rows: [ServerRow]) -> [String] {
    ["\(rows.count)", "\(sessionCount(rows))", memText(rows.reduce(0) { $0 + $1.s.mem }), cpuText(totalCPU(rows))]
}

// MARK: - Text

/// "0.6%", "143%" from 100, "—" before a second sample.
func cpuText(_ p: Double?) -> String { p.map { String(format: $0 >= 100 ? "%.0f%%" : "%.1f%%", $0) } ?? "—" }

/// "8.2 MB" under 10 MB, "49 MB", "1.2 GB" from 1024 MB.
func memText(_ b: UInt64) -> String {
    let mb = Double(b) / 1_048_576
    return mb >= 1024 ? String(format: "%.1f GB", mb / 1024) : String(format: mb < 10 ? "%.1f MB" : "%.0f MB", mb)
}

/// The footer's line about the tray itself, from two (CPU ns, time) samples of its own usage.
func trayStats(_ prev: (cpu: UInt64, at: Date)?, cpu: UInt64, mem: UInt64, at: Date) -> String {
    let pct = prev.flatMap { p in at > p.at ? Double(cpu > p.cpu ? cpu - p.cpu : 0) / 1e9 / at.timeIntervalSince(p.at) * 100 : nil }
    return "Tray: CPU \(cpuText(pct)) · Memory \(memText(mem))"
}

/// --cli <path> from the LaunchAgent; "" without it, so no build shows "installed".
func cliPath(_ args: [String]) -> String {
    if let i = args.firstIndex(of: "--cli"), i + 1 < args.count { return args[i + 1] }
    return ""
}

/// --tailscale host:port=login from the LaunchAgent; "" without one.
func tailscaleArg(_ args: [String]) -> String {
    if let i = args.firstIndex(of: "--tailscale"), i + 1 < args.count { return args[i + 1] }
    return ""
}

/// The dashboard through `tailscale serve`: https://host:port; nil unless --tailscale is host:port=login.
func tailnetURL(_ ts: String) -> URL? {
    let parts = ts.components(separatedBy: "=")
    guard parts.count == 2, !parts[0].isEmpty, !parts[1].isEmpty else { return nil }
    return URL(string: "https://\(parts[0])")
}

/// `trello-mcp dashboard`'s arguments, with the tray's --tailscale.
func dashboardArgs(_ ts: String) -> [String] { ["dashboard"] + (ts.isEmpty ? [] : ["--tailscale", ts]) }

/// Seconds before restarting a dashboard that exited: 1, doubling to 60 while it keeps dying
/// young, 1 again once it ran 5 minutes (a busy port or a bad build must not spin).
func restartDelay(_ last: TimeInterval?, uptime: TimeInterval) -> TimeInterval {
    guard let last, uptime < 300 else { return 1 }
    return min(last * 2, 60)
}

/// `p` with symlinks resolved, as proc_pidpath reports binaries; `p` when it does not exist.
/// realpath, not URL.resolvingSymlinksInPath, which drops a leading /private.
func realPath(_ p: String) -> String {
    guard !p.isEmpty, let r = realpath(p, nil) else { return p }
    defer { free(r) }
    return String(cString: r)
}

// MARK: - libproc

/// CPU times come in Mach ticks, not ns, on Apple silicon.
let timebase: mach_timebase_info_data_t = {
    var t = mach_timebase_info_data_t()
    mach_timebase_info(&t)
    return t
}()

/// The size of the argument area, for KERN_PROCARGS2.
let argMax: Int = {
    var n: Int32 = 0, size = MemoryLayout<Int32>.size, mib = [CTL_KERN, KERN_ARGMAX]
    return sysctl(&mib, 2, &n, &size, nil, 0) == 0 ? Int(n) : 1 << 20
}()

/// This user's pids: the only processes the tray may stop.
func userPids() -> [pid_t] {
    let type = UInt32(PROC_UID_ONLY), uid = UInt32(getuid())
    let bytes = proc_listpids(type, uid, nil, 0)
    guard bytes > 0 else { return [] }
    var pids = [pid_t](repeating: 0, count: Int(bytes) / MemoryLayout<pid_t>.size + 64)   // room for processes started since
    let got = pids.withUnsafeMutableBytes { proc_listpids(type, uid, $0.baseAddress, Int32($0.count)) }
    return pids.prefix(Int(max(got, 0)) / MemoryLayout<pid_t>.size).filter { $0 > 0 }
}

func procName(_ pid: pid_t) -> String? {
    var b = [CChar](repeating: 0, count: 64)   // proc_name fills up to 2 * MAXCOMLEN
    return proc_name(pid, &b, UInt32(b.count)) > 0 ? String(cString: b) : nil
}

/// Parent pid and start time (µs); nil once the process is gone.
func bsdInfo(_ pid: pid_t) -> (ppid: pid_t, start: UInt64)? {
    var i = proc_bsdinfo()
    let size = Int32(MemoryLayout<proc_bsdinfo>.size)
    guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &i, size) == size else { return nil }
    return (pid_t(i.pbi_ppid), i.pbi_start_tvsec * 1_000_000 + i.pbi_start_tvusec)
}

func folder(of pid: pid_t) -> String? {
    var v = proc_vnodepathinfo()
    let size = Int32(MemoryLayout<proc_vnodepathinfo>.size)
    guard proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, &v, size) == size else { return nil }
    var p = v.pvi_cdir.vip_path
    return withUnsafeBytes(of: &p) { String(cString: $0.baseAddress!.assumingMemoryBound(to: CChar.self)) }
}

func path(of pid: pid_t) -> String? {
    var b = [CChar](repeating: 0, count: 4096)   // PROC_PIDPATHINFO_MAXSIZE
    return proc_pidpath(pid, &b, UInt32(b.count)) > 0 ? String(cString: b) : nil
}

func args(of pid: pid_t) -> [String]? {
    var mib = [CTL_KERN, KERN_PROCARGS2, pid], size = argMax
    var b = [UInt8](repeating: 0, count: size)
    guard sysctl(&mib, 3, &b, &size, nil, 0) == 0 else { return nil }
    return parseProcArgs(Array(b.prefix(size)))
}

/// CPU time in ns and memory footprint in bytes, as Activity Monitor counts them; nil once it is gone.
func usage(of pid: pid_t) -> (cpu: UInt64, mem: UInt64)? {
    var ri = rusage_info_v2()
    let rc = withUnsafeMutablePointer(to: &ri) {
        $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V2, $0) }
    }
    guard rc == 0 else { return nil }
    return ((ri.ri_user_time + ri.ri_system_time) * UInt64(timebase.numer) / UInt64(timebase.denom), ri.ri_phys_footprint)
}

/// Finds and measures the running servers. What never changes for a process (its binary and its
/// parent's start time and folder) is read once, so a tick reads only names, parents and usage.
/// Not thread-safe: the tray runs it on one serial queue, one scan at a time.
final class Scanner {
    private struct Fixed { let path: String; let ppid: pid_t; let parentStarted: UInt64?; let folder: String? }
    private var fixed: [ServerID: Fixed] = [:]
    private var others: Set<ServerID> = []      // trello-mcp processes that are not servers

    func scan() -> [ServerSample] {
        var out: [ServerSample] = [], nextFixed: [ServerID: Fixed] = [:], nextOthers: Set<ServerID> = []
        for pid in userPids() where procName(pid) == serverName {
            // A failing call means the process just exited: skip it.
            guard let b = bsdInfo(pid) else { continue }
            let id = ServerID(pid: pid, start: b.start)
            if others.contains(id) { nextOthers.insert(id); continue }
            var f = fixed[id]
            if f == nil {
                guard let argv = args(of: pid) else { continue }
                guard isServer(argv) else { nextOthers.insert(id); continue }
                // The session is the parent; launchd (1) or a parent that is gone means none.
                let session = b.ppid > 1 ? bsdInfo(b.ppid) : nil
                f = Fixed(path: path(of: pid) ?? argv.first ?? serverName, ppid: b.ppid, parentStarted: session?.start, folder: session == nil ? nil : folder(of: b.ppid))
            }
            guard let f, let u = usage(of: pid) else { continue }
            nextFixed[id] = f
            // A server whose parent exited is adopted by launchd: it has no session from then on.
            let session = b.ppid == f.ppid
            out.append(ServerSample(id: id, ppid: b.ppid, parentStarted: session ? f.parentStarted : nil,
                                    folder: session ? f.folder : nil, path: f.path, cpuNs: u.cpu, mem: u.mem))
        }
        fixed = nextFixed
        others = nextOthers
        return out
    }
}

// MARK: - Stop

/// The process still runs under this pid and start time, with this name.
func alive(_ id: ServerID, name: String) -> Bool { procName(id.pid) == name && bsdInfo(id.pid)?.start == id.start }

/// Why kill failed; nil when the process was already gone.
func stopFailure(_ pid: pid_t, _ e: Int32) -> String? {
    e == ESRCH ? nil : "Could not stop PID \(pid): \(String(cString: strerror(e)))"
}

/// Seconds between SIGTERM and SIGKILL.
let stopGrace: TimeInterval = 5

/// Stops `id` if it still runs as `name`: SIGTERM, then SIGKILL after `grace` s if it lives on.
/// done gets nil once it is stopped or gone, else why not: at once when nothing was sent, else on
/// the main queue after the grace period.
func stop(_ id: ServerID, name: String, grace: TimeInterval = stopGrace, done: @escaping (String?) -> Void) {
    guard alive(id, name: name) else { return done(nil) }   // gone, or its pid now belongs to another process
    if kill(id.pid, SIGTERM) != 0 { return done(stopFailure(id.pid, errno)) }
    DispatchQueue.main.asyncAfter(deadline: .now() + grace) {
        if alive(id, name: name) { kill(id.pid, SIGKILL) }
        done(nil)
    }
}
