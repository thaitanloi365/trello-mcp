// Finding and measuring the running trello-mcp servers with libproc, stopping one, and the pure
// rules and text the panel shows them by. No AppKit here; main.swift's SELFTEST block checks it all.
import Foundation

/// The server binary's process name. The tray itself is trello-mcp-tray, so it never matches.
let serverName = "trello-mcp"
let userHome = FileManager.default.homeDirectoryForCurrentUser.path

/// A process's identity: the start time tells a reused pid apart.
struct ServerID: Hashable {
    let pid: pid_t
    let start: UInt64   // µs since 1970

    var started: Date { Date(timeIntervalSince1970: Double(start) / 1e6) }
}

/// One measured server.
struct Proc: Equatable {
    let id: ServerID
    let ppid: pid_t
    let parent: String      // the parent's name (basename of its argv[0]), "" when unknown
    let folder: String?     // working folder: the project the client started it in
    let binary: String
    let config: String?     // the --config value
    let cpuNs: UInt64       // CPU time so far
    let mem: UInt64         // physical footprint in bytes: Activity Monitor's Memory
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

func configArg(_ argv: [String]) -> String? {
    for (i, a) in argv.enumerated().dropFirst() {
        if a == "--config" { return i + 1 < argv.count ? argv[i + 1] : nil }
        if a.hasPrefix("--config=") { return String(a.dropFirst("--config=".count)) }
    }
    return nil
}

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

// MARK: - Labels and status

/// Each working folder's last component, after its parent when the name is shorter than 4
/// characters or another folder has the same name. Home is ~; an unknown folder is trello-mcp.
func projectLabels(_ folders: [String?], home: String) -> [String] {
    let name = { (f: String) in (f as NSString).lastPathComponent }
    var uses: [String: Int] = [:]
    for f in Set(folders.compactMap { $0 }) { uses[name(f), default: 0] += 1 }
    return folders.map { f in
        guard let f else { return serverName }
        if f == "/" { return "/" }
        if f == home { return "~" }
        let n = name(f)
        guard n.count < 4 || uses[n, default: 0] > 1 else { return n }
        let parent = (f as NSString).deletingLastPathComponent
        return parent == home ? "~/" + n : name(parent) + "/" + n
    }
}

func tilde(_ path: String, home: String) -> String {
    path == home ? "~" : path.hasPrefix(home + "/") ? "~" + path.dropFirst(home.count) : path
}

enum Status: Equatable { case stopping, orphaned, busy, idle }

/// Orphaned: the client that started it is gone and launchd adopted it.
func status(ppid: pid_t, cpu: Double?, stopping: Bool) -> Status {
    stopping ? .stopping : ppid == 1 ? .orphaned : (cpu ?? 0) > 0.05 ? .busy : .idle
}

enum Tone: Equatable { case ink, amber, red }

/// The Threshold Rule: ink below 80%, amber from 80%, red from 95%.
func tone(_ p: Double?) -> Tone { (p ?? 0) >= 95 ? .red : (p ?? 0) >= 80 ? .amber : .ink }

/// A server as the panel shows it.
struct Row: Equatable {
    let proc: Proc
    let label: String
    let cpu: Double?    // percent of one core over the last interval; nil before a second sample
    let status: Status
    var id: ServerID { proc.id }
    var mem: UInt64 { proc.mem }
}

/// Rows by project label, then start time: the order never moves as numbers tick.
func rows(_ procs: [Proc], cpu: [ServerID: Double], stopping: Set<ServerID>, home: String) -> [Row] {
    zip(procs, projectLabels(procs.map(\.folder), home: home)).map { p, label in
        Row(proc: p, label: label, cpu: cpu[p.id], status: status(ppid: p.ppid, cpu: cpu[p.id], stopping: stopping.contains(p.id)))
    }.sorted { ($0.label, $0.id.start, $0.id.pid) < ($1.label, $1.id.start, $1.id.pid) }
}

/// Same servers, labels and stopping state: a tick updates numbers in place instead of re-rendering.
func sameShape(_ a: [Row], _ b: [Row]) -> Bool {
    a.map(\.id) == b.map(\.id) && a.map(\.label) == b.map(\.label) && a.map { $0.status == .stopping } == b.map { $0.status == .stopping }
}

// MARK: - Text

/// Percent of one core between two (CPU ns, uptime s) samples; nil without an earlier one.
func cpuPercent(_ a: (cpu: UInt64, at: TimeInterval)?, _ b: (cpu: UInt64, at: TimeInterval)) -> Double? {
    guard let a, b.at > a.at else { return nil }
    return max(0, Double(b.cpu) - Double(a.cpu)) / 1e9 / (b.at - a.at) * 100
}

/// "0.6%", "143%" from 100, "—" before a second sample.
func cpuText(_ p: Double?) -> String { p.map { String(format: $0 >= 100 ? "%.0f%%" : "%.1f%%", $0) } ?? "—" }

/// "8.2 MB" under 10 MB, "49 MB", "1.2 GB" from 1024 MB.
func memText(_ b: UInt64) -> String {
    let mb = Double(b) / 1_048_576
    return mb >= 1024 ? String(format: "%.1f GB", mb / 1024) : String(format: mb < 10 ? "%.1f MB" : "%.0f MB", mb)
}

/// "4 MIN", "2H 14M", "1 DAY", "3 DAYS", rounded down.
func uptime(_ t: TimeInterval) -> String {
    let m = max(0, Int(t / 60)), d = m / 1440
    if d > 0 { return "\(d) DAY\(d == 1 ? "" : "S")" }
    return m >= 60 ? String(format: "%dH %02dM", m / 60, m % 60) : "\(m) MIN"
}

/// "IDLE · PID 3959 · UP 3 DAYS"
func statusLine(_ r: Row, now: Date) -> String {
    let word = switch r.status {
    case .stopping: "STOPPING…"
    case .orphaned: "ORPHANED"
    case .busy: "BUSY"
    case .idle: "IDLE"
    }
    return "\(word) · PID \(r.id.pid) · UP \(uptime(now.timeIntervalSince(r.id.started)))"
}

/// VoiceOver text of a row.
func rowSummary(_ r: Row, now: Date) -> String {
    "\(r.label), CPU \(cpuText(r.cpu)), memory \(memText(r.mem)), \(statusLine(r, now: now).lowercased())"
}

/// Summed CPU (nil until a server has a second sample) and memory, and the note under them.
func totals(_ rows: [Row], now: Date) -> (cpu: Double?, mem: UInt64, note: String) {
    let cpus = rows.compactMap(\.cpu)
    var parts: [String] = []
    for (s, word) in [(Status.idle, "idle"), (.busy, "busy"), (.orphaned, "orphaned"), (.stopping, "stopping")] {
        let n = rows.filter { $0.status == s }.count
        if n > 0 { parts.append("\(n) \(word)") }
    }
    if let first = rows.map(\.id.started).min() { parts.append("oldest up " + uptime(now.timeIntervalSince(first)).lowercased()) }
    return (cpus.isEmpty ? nil : cpus.reduce(0, +), rows.reduce(0) { $0 + $1.mem }, parts.joined(separator: " · "))
}

/// "Today 9:12 AM", "Mon 9:12 AM" within a week, else "Oct 6 9:12 AM".
func startedText(_ d: Date, now: Date) -> String {
    let f = DateFormatter()
    f.dateFormat = "h:mm a"
    if Calendar.current.isDate(d, inSameDayAs: now) { return "Today " + f.string(from: d) }
    f.dateFormat = now.timeIntervalSince(d) < 6 * 86400 ? "EEE h:mm a" : "MMM d h:mm a"
    return f.string(from: d)
}

/// Label and value lines of an expanded row.
func detailLines(_ p: Proc, home: String, now: Date) -> [(String, String)] {
    [("Folder", p.folder.map { tilde($0, home: home) } ?? "unknown"),
     ("Binary", tilde(p.binary, home: home)),
     ("Config", p.config.map { tilde($0, home: home) } ?? "default"),
     ("Started by", p.ppid == 1 ? "launchd · orphaned" : "\(p.parent.isEmpty ? "unknown" : p.parent) · PID \(p.ppid)"),
     ("Started", startedText(p.id.started, now: now))]
}

func trayStats(cpu: Double?, mem: UInt64) -> String { "Tray: CPU \(cpuText(cpu)) · Memory \(memText(mem))" }

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

/// Finds and measures the running servers. What never changes for a process (its arguments,
/// binary, folder and parent name) is read once, so a tick reads only names and usage.
final class Scanner {
    private struct Fixed { let binary: String; let folder: String?; let config: String?; let parent: String }
    private var fixed: [ServerID: Fixed] = [:]
    private var others: Set<ServerID> = []      // trello-mcp processes that are not servers

    func scan() -> [Proc] {
        var out: [Proc] = [], nextFixed: [ServerID: Fixed] = [:], nextOthers: Set<ServerID> = []
        for pid in userPids() where procName(pid) == serverName {
            // A failing call means the process just exited: skip it.
            guard let b = bsdInfo(pid) else { continue }
            let id = ServerID(pid: pid, start: b.start)
            if others.contains(id) { nextOthers.insert(id); continue }
            var f = fixed[id]
            if f == nil {
                guard let argv = args(of: pid) else { continue }
                guard isServer(argv) else { nextOthers.insert(id); continue }
                f = Fixed(binary: path(of: pid) ?? argv.first ?? serverName, folder: folder(of: pid), config: configArg(argv),
                          // argv[0], not proc_name: Claude Code's executable file is named after its version
                          parent: args(of: b.ppid)?.first.map { ($0 as NSString).lastPathComponent } ?? procName(b.ppid) ?? "")
            }
            guard let f, let u = usage(of: pid) else { continue }
            nextFixed[id] = f
            out.append(Proc(id: id, ppid: b.ppid, parent: f.parent, folder: f.folder, binary: f.binary,
                            config: f.config, cpuNs: u.cpu, mem: u.mem))
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

/// Stops `id` if it still runs as `name`: SIGTERM, then SIGKILL after `grace` s if it lives on.
/// done gets nil once it is stopped or gone, else why not: at once when nothing was sent, else on
/// the main queue after the grace period.
func stop(_ id: ServerID, name: String, grace: TimeInterval = 3, done: @escaping (String?) -> Void) {
    guard alive(id, name: name) else { return done(nil) }   // gone, or its pid now belongs to another process
    if kill(id.pid, SIGTERM) != 0 { return done(stopFailure(id.pid, errno)) }
    DispatchQueue.main.asyncAfter(deadline: .now() + grace) {
        if alive(id, name: name) { kill(id.pid, SIGKILL) }
        done(nil)
    }
}
