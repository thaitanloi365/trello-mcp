// trello-mcp-tray: a menu bar item whose panel lists the running trello-mcp servers with their
// CPU and memory and can stop one. It samples only while the panel shows (libproc, 0.5 s after
// opening, then every 2 s) and sits idle otherwise. `make tray-install` starts it at login through
// ~/Library/LaunchAgents/com.trellomcp.tray.plist, which points stdout/stderr at
// ~/Library/Logs/trello-mcp-tray.log.
import AppKit

let logURL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/trello-mcp-tray.log")

/// Menu bar icon: the board mark as a template image, so it follows the menu bar's light and dark.
func boardImage() -> NSImage {
    let h: CGFloat = 16, w = h / 4, gap = h / 8
    let img = NSImage(size: NSSize(width: 3 * w + 2 * gap, height: h), flipped: false) { _ in
        NSColor.black.setFill()
        for (i, k) in boardColumns.enumerated() {
            NSBezierPath(rect: NSRect(x: CGFloat(i) * (w + gap), y: h * (1 - k), width: w, height: h * k)).fill()   // hang from the top
        }
        return true
    }
    img.isTemplate = true
    return img
}

#if SELFTEST
// The fake server below is this binary copied as trello-mcp: copies of system binaries (arm64e) are killed on launch.
if CommandLine.arguments.count > 1, CommandLine.arguments[1] == "serve" { while true { sleep(60) } }

// Which trello-mcp processes are servers.
precondition(isServer(["/x/trello-mcp", "serve"]) && isServer(["/x/trello-mcp"]), "serve, no arguments")
precondition(isServer(["trello-mcp", "--config", "/a b/c.json", "serve"]) && isServer(["trello-mcp", "serve", "--config", "/c.json"]), "--config around serve")
precondition(isServer(["trello-mcp", "--config=/c.json"]) && isServer(["trello-mcp", "--config", "/c.json"]), "--config alone still serves")
precondition(!isServer(["trello-mcp", "config", "show"]) && !isServer(["trello-mcp", "version"]) && !isServer(["trello-mcp", "-v"])
    && !isServer(["trello-mcp", "--config", "/c.json", "update"]), "other commands are not servers")
precondition(configArg(["trello-mcp", "serve", "--config", "/a b/c.json"]) == "/a b/c.json" && configArg(["trello-mcp", "--config=/c.json"]) == "/c.json"
    && configArg(["trello-mcp", "serve"]) == nil && configArg(["trello-mcp", "--config"]) == nil, "config value")

// KERN_PROCARGS2: argc, exec path, NUL padding, argv, then the environment.
let argc4 = withUnsafeBytes(of: Int32(4)) { Array($0) }
let buf = argc4 + Array("/My Apps/trello-mcp\0\0\0\0trello-mcp\0serve\0--config\0/a b/c.json\0HOME=/Users/me\0".utf8)
precondition(parseProcArgs(buf) == ["trello-mcp", "serve", "--config", "/a b/c.json"], "procargs: \(String(describing: parseProcArgs(buf)))")
precondition(parseProcArgs([1, 0]) == nil, "procargs: shorter than argc")
precondition(parseProcArgs(withUnsafeBytes(of: Int32(3)) { Array($0) } + Array("/x\0\0a\0".utf8)) == nil, "procargs: fewer strings than argc")
precondition(parseProcArgs(withUnsafeBytes(of: Int32(1)) { Array($0) } + Array("/x\0\0abc".utf8)) == nil, "procargs: cut before its NUL")

// Project labels.
let h = "/Users/me"
let labels = projectLabels(["/Users/me/P/scoutlink_backend", "/Users/me/P/tonotopia/app", h, nil, "/",
                            "/Users/me/x/server", "/Users/me/y/server", "/Users/me/x/server", "/Users/me/app"], home: h)
precondition(labels == ["scoutlink_backend", "tonotopia/app", "~", "trello-mcp", "/", "x/server", "y/server", "x/server", "~/app"], "labels: \(labels)")
precondition(projectLabels(["/a/eventcreate_app", "/a/eventcreate_app"], home: h) == ["eventcreate_app", "eventcreate_app"], "one folder twice is no clash")
precondition(tilde("/Users/me/x", home: h) == "~/x" && tilde(h, home: h) == "~" && tilde("/Users/meow/x", home: h) == "/Users/meow/x", "tilde")

// Rows: order, status, totals, shape.
let t0: UInt64 = 1_791_800_000_000_000
func proc(_ pid: pid_t, _ folder: String?, start: UInt64, ppid: pid_t = 500) -> Proc {
    Proc(id: ServerID(pid: pid, start: start), ppid: ppid, parent: "claude", folder: folder, binary: "/Users/me/bin/trello-mcp",
         config: nil, cpuNs: 0, mem: 8 << 20)
}
let ps = [proc(30, "/Users/me/P/ourblock", start: t0), proc(10, "/Users/me/P/eventcreate_app", start: t0 + 2),
          proc(20, "/Users/me/P/eventcreate_app", start: t0 + 1, ppid: 1), proc(40, "/Users/me/P/ahai", start: t0)]
let now = Date(timeIntervalSince1970: Double(t0) / 1e6 + 3 * 86400 + 60)
let stoppingSet: Set<ServerID> = [ServerID(pid: 30, start: t0)]
let rs = rows(ps, cpu: [ServerID(pid: 10, start: t0 + 2): 0.6, ServerID(pid: 40, start: t0): 96.8], stopping: stoppingSet, home: h)
precondition(rs.map(\.id.pid) == [40, 20, 10, 30], "order: label, then start: \(rs.map(\.id.pid))")
precondition(rs.map(\.status) == [.busy, .orphaned, .busy, .stopping], "status: \(rs.map(\.status))")
precondition(rs[1].cpu == nil && rs[0].cpu == 96.8, "cpu per row")
precondition(status(ppid: 500, cpu: 0.04, stopping: false) == .idle && status(ppid: 500, cpu: nil, stopping: false) == .idle
    && status(ppid: 500, cpu: 0.06, stopping: false) == .busy && status(ppid: 1, cpu: 50, stopping: true) == .stopping, "status edges")
precondition(statusLine(rs[1], now: now) == "ORPHANED · PID 20 · UP 3 DAYS", "status line: \(statusLine(rs[1], now: now))")
precondition(rowSummary(rs[1], now: now) == "eventcreate_app, CPU —, memory 8.0 MB, orphaned · pid 20 · up 3 days", "summary: \(rowSummary(rs[1], now: now))")
let tt = totals(rs, now: now)
precondition(abs(tt.cpu! - 97.4) < 1e-9 && tt.mem == 32 << 20, "totals: \(tt)")
precondition(tt.note == "2 busy · 1 orphaned · 1 stopping · oldest up 3 days", "note: \(tt.note)")
precondition(totals([], now: now).cpu == nil && totals([], now: now).note == "", "no servers")
precondition(sameShape(rs, rows(ps, cpu: [:], stopping: stoppingSet, home: h)), "new numbers keep the shape")
precondition(!sameShape(rs, rows(ps, cpu: [:], stopping: [], home: h)), "a stop ending changes the shape")
precondition(!sameShape(rs, Array(rs.dropLast())), "a server leaving changes the shape")

// Text.
precondition(uptime(4 * 60 + 59) == "4 MIN" && uptime(2 * 3600 + 14 * 60) == "2H 14M" && uptime(6 * 3600 + 2 * 60) == "6H 02M", "uptime")
precondition(uptime(86400) == "1 DAY" && uptime(3 * 86400 + 5) == "3 DAYS" && uptime(-5) == "0 MIN", "uptime: days, clock skew")
precondition(memText(8_598_323) == "8.2 MB" && memText(49 << 20) == "49 MB" && memText(1229 << 20) == "1.2 GB", "mem: \(memText(8_598_323))")
precondition(cpuText(nil) == "—" && cpuText(0.04) == "0.0%" && cpuText(96.84) == "96.8%" && cpuText(143.2) == "143%", "cpu text")
precondition(tone(79.9) == .ink && tone(80) == .amber && tone(94.9) == .amber && tone(95) == .red && tone(nil) == .ink, "threshold rule")
precondition(cpuPercent(nil, (5, 1)) == nil && cpuPercent((0, 1), (5, 1)) == nil, "cpu: two samples, time apart")
precondition(cpuPercent((1_000_000_000, 10), (1_500_000_000, 12)) == 25, "cpu: 0.5 s over 2 s is 25%")
precondition(cpuPercent((9, 10), (1, 12)) == 0, "cpu is never negative")
precondition(trayStats(cpu: nil, mem: 21 << 20) == "Tray: CPU — · Memory 21 MB", "tray stats")
let d = detailLines(Proc(id: ServerID(pid: 4648, start: t0), ppid: 3614, parent: "claude", folder: "/Users/me/P/scoutlink_backend",
                         binary: "/Users/me/ai-tools/trello-mcp/bin/trello-mcp", config: "/Users/me/.trello-mcp/config.json", cpuNs: 0, mem: 0),
                    home: h, now: now)
precondition(d.map(\.0) == ["Folder", "Binary", "Config", "Started by", "Started"], "detail keys")
precondition(d[0].1 == "~/P/scoutlink_backend" && d[1].1 == "~/ai-tools/trello-mcp/bin/trello-mcp" && d[2].1 == "~/.trello-mcp/config.json"
    && d[3].1 == "claude · PID 3614", "details: \(d)")
let orphan = detailLines(rs[1].proc, home: h, now: now)
precondition(orphan[2].1 == "default" && orphan[3].1 == "launchd · orphaned", "orphan details: \(orphan)")
precondition(startedText(now.addingTimeInterval(-60), now: now).hasPrefix("Today "), "started today")
precondition(stopFailure(4648, EPERM) == "Could not stop PID 4648: Operation not permitted" && stopFailure(4648, ESRCH) == nil, "stop failure text")

// libproc on this process.
guard let me = bsdInfo(getpid()) else { fatalError("own bsd info") }
precondition(me.ppid == getppid() && me.start > 0, "bsd info")
precondition(procName(getpid())?.hasPrefix("trello-mcp-tray") == true, "own name: \(String(describing: procName(getpid())))")
precondition(folder(of: getpid()) == FileManager.default.currentDirectoryPath, "own folder: \(String(describing: folder(of: getpid())))")
precondition(args(of: getpid()) == CommandLine.arguments, "own args")
precondition(userPids().contains(getpid()), "own pid is listed")
guard let u0 = usage(of: getpid()) else { fatalError("own usage reads") }
let spin = Date()
var n = 0
while Date().timeIntervalSince(spin) < 0.3 { n &+= 1 }
let burnt = Double(usage(of: getpid())!.cpu - u0.cpu) / 1e9
precondition(u0.mem > 1 << 20 && (0.15...0.6).contains(burnt), "usage: \(u0.mem) B, \(burnt) s CPU for 0.3 s busy (ticks converted)")
precondition(usage(of: -1) == nil && bsdInfo(-1) == nil && procName(-1) == nil, "no process")

// A fake server: this binary copied as trello-mcp into a folder with a space, run as `serve`.
let dir = FileManager.default.temporaryDirectory.appendingPathComponent("tray selftest \(getpid())")
try! FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
let fake = dir.appendingPathComponent("trello-mcp")
try! FileManager.default.copyItem(at: URL(fileURLWithPath: CommandLine.arguments[0]), to: fake)
let child = Process()
child.executableURL = fake
child.currentDirectoryURL = dir
child.arguments = ["serve"]
try! child.run()
let sc = Scanner()
var found: Proc?
for _ in 0..<100 where found == nil {
    found = sc.scan().first { $0.id.pid == child.processIdentifier }
    if found == nil { usleep(10_000) }
}
guard let f = found else { fatalError("the fake server is found") }
precondition(f.binary.hasSuffix("/tray selftest \(getpid())/trello-mcp") && f.folder?.hasSuffix("/tray selftest \(getpid())") == true, "paths with a space: \(f)")
precondition(f.ppid == getpid() && f.parent.hasPrefix("trello-mcp-tray") && f.config == nil && f.mem > 0, "parent, config, memory: \(f)")
var t = Date()
_ = sc.scan()
let warm = Date().timeIntervalSince(t)
t = Date()
_ = Scanner().scan()
let cold = Date().timeIntervalSince(t)
print(String(format: "scan: %.1f ms cached, %.1f ms cold, %d pids", warm * 1000, cold * 1000, userPids().count))
precondition(warm < 0.05, "a cached scan stays cheap")

// Stop: identity checks first, then SIGTERM.
var answered = false
stop(ServerID(pid: f.id.pid, start: f.id.start + 1), name: serverName, grace: 0.1) { precondition($0 == nil); answered = true }
precondition(answered && child.isRunning, "a reused pid (other start time) is never signalled")
answered = false
stop(f.id, name: "sleep", grace: 0.1) { precondition($0 == nil); answered = true }
precondition(answered && child.isRunning, "another name is never signalled")
answered = false
stop(f.id, name: serverName, grace: 0.3) { precondition($0 == nil, "stop: \($0 ?? "")"); answered = true }
child.waitUntilExit()
RunLoop.main.run(until: Date().addingTimeInterval(0.5))
precondition(answered && child.terminationReason == .uncaughtSignal && child.terminationStatus == SIGTERM, "stopped by SIGTERM")
precondition(sc.scan().first { $0.id == f.id } == nil, "a stopped server leaves the scan")

// A process that ignores SIGTERM gets SIGKILL after the grace period.
let stubborn = Process()
stubborn.executableURL = URL(fileURLWithPath: "/bin/sh")
stubborn.arguments = ["-c", "trap '' TERM; exec /bin/sleep 30"]
try! stubborn.run()
for _ in 0..<100 where procName(stubborn.processIdentifier) != "sleep" { usleep(10_000) }
let sid = ServerID(pid: stubborn.processIdentifier, start: bsdInfo(stubborn.processIdentifier)!.start)
answered = false
stop(sid, name: "sleep", grace: 0.3) { _ in answered = true }
RunLoop.main.run(until: Date().addingTimeInterval(0.6))
stubborn.waitUntilExit()
precondition(answered && stubborn.terminationReason == .uncaughtSignal && stubborn.terminationStatus == SIGKILL, "SIGKILL after the grace period")
try? FileManager.default.removeItem(at: dir)

// Menu bar icon and panel rendering (no window: sizes only).
precondition(boardImage().isTemplate && boardImage().size == NSSize(width: 16, height: 16), "menu bar icon: \(boardImage().size)")
_ = NSApplication.shared
let pm = PanelModel(), pc = PanelController(pm)
func panelSize() -> NSSize { pc.render(); return pc.view.subviews.first?.fittingSize ?? .zero }
pm.stats = trayStats(cpu: 0.1, mem: 21 << 20)
pm.rows = rs
pm.open = rs[1].id
pm.error = stopFailure(20, EPERM)
precondition(panelSize().width == 360, "panel with rows, an open row and an error: \(panelSize())")
let wide = Proc(id: ServerID(pid: 99999, start: t0), ppid: 500, parent: "claude", folder: "/Users/me/" + String(repeating: "very_long_project_name_", count: 3),
                binary: "/Users/me/bin/trello-mcp", config: nil, cpuNs: 0, mem: 1229 << 20)
pm.rows = rows([wide], cpu: [wide.id: 143.2], stopping: [], home: h)
precondition(panelSize().width == 360, "long label, 143% and 1.2 GB keep the width: \(panelSize())")
pm.rows = (1...40).map { i in Row(proc: proc(pid_t(1000 + i), "/Users/me/P/project\(i)", start: t0 + UInt64(i)), label: "project\(i)", cpu: 0, status: .idle) }
pm.cap = 400
precondition(panelSize().height < 400 + 220, "40 rows scroll inside the cap: \(panelSize())")
pm.rows = []
pm.error = nil
precondition(panelSize().width == 360, "empty state")
pc.view.appearance = NSAppearance(named: .darkAqua)
pm.rows = rs
precondition(panelSize().width == 360, "dark")

print("tray selftest ok")
exit(0)
#endif

/// The panel's window, under the status item. Borderless, not an NSPopover: the popover's window
/// alone took 13 MB in claude-usage. Esc, a click elsewhere or another app taking the keyboard
/// closes it. Non-activating: macOS refuses to activate a menu bar app on a status item click, so
/// the panel takes the keyboard without activation, and must not hide while the app is inactive.
final class PanelWindow: NSPanel {
    var onClose: () -> Void = {}
    override var canBecomeKey: Bool { true }
    override func resignKey() { super.resignKey(); onClose() }
    override func cancelOperation(_ sender: Any?) { onClose() }
}

final class Tray: NSObject, NSApplicationDelegate {
    var item: NSStatusItem!
    let model = PanelModel()
    lazy var panel = PanelController(model)
    let window = PanelWindow(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
    let scanner = Scanner()
    var clicks: Any?            // watches clicks in other apps while the panel shows
    var timer: Timer?           // the next sample, only while the panel shows
    var last: [ServerID: (cpu: UInt64, at: TimeInterval)] = [:]    // each server's previous sample
    var mine: (cpu: UInt64, at: TimeInterval)?                      // the tray's own previous sample
    var stopping: Set<ServerID> = []

    func applicationDidFinishLaunching(_ n: Notification) {
        item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = boardImage()
        item.button?.toolTip = "Trello MCP"
        item.button?.setAccessibilityLabel("Trello MCP")
        item.button?.target = self
        item.button?.action = #selector(togglePanel)
        model.onStop = { [unowned self] in stopServer($0) }
        model.onLog = { [unowned self] in closePanel(); NSWorkspace.shared.open(logURL) }
        model.onQuit = { exit(0) }      // exit 0: the LaunchAgent leaves it down until the next login
        window.contentViewController = panel
        window.level = .popUpMenu
        window.hasShadow = true
        window.hidesOnDeactivate = false
        window.isReleasedWhenClosed = false
        window.onClose = { [unowned self] in closePanel() }
    }

    @objc func togglePanel() { if window.isVisible { closePanel() } else { openPanel() } }

    func openPanel() {
        guard let b = item.button, let bar = b.window else { return }
        let r = bar.convertToScreen(b.convert(b.bounds, to: nil)), vis = (bar.screen ?? NSScreen.main)?.visibleFrame ?? .zero
        model.cap = vis.height - 160    // room for header and footer
        last = [:]                      // CPU since the panel last closed would be a stale average
        mine = nil
        sample()
        panel.render()                  // sizes the window
        let s = window.frame.size
        window.setFrameOrigin(NSPoint(x: min(max(r.midX - s.width / 2, vis.minX), vis.maxX - s.width), y: bar.frame.minY - s.height))
        window.makeKeyAndOrderFront(nil)
        // The status item's own clicks reach us as global events; its action does the toggling.
        clicks = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [unowned self] e in
            if !r.contains(e.locationInWindow) { closePanel() }
        }
        sampleAgain(after: 0.5)         // CPU needs a second sample: show it almost at once
    }

    /// Samples after `delay`, then every 2 s while the panel shows. The tolerance lets macOS batch wakeups.
    func sampleAgain(after delay: TimeInterval) {
        let t = Timer(timeInterval: delay, repeats: false) { [unowned self] _ in
            sample()
            sampleAgain(after: 2)
        }
        t.tolerance = 0.2
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func closePanel() {
        guard window.isVisible else { return }
        window.orderOut(nil)
        if let c = clicks { NSEvent.removeMonitor(c) }
        clicks = nil
        timer?.invalidate()
        timer = nil
        model.error = nil
        model.open = nil
    }

    /// Reads the servers and the tray's own usage and hands them to the panel.
    func sample() {
        let at = ProcessInfo.processInfo.systemUptime
        let procs = scanner.scan()
        var cpu: [ServerID: Double] = [:], next: [ServerID: (cpu: UInt64, at: TimeInterval)] = [:]
        for p in procs {
            next[p.id] = (p.cpuNs, at)
            cpu[p.id] = cpuPercent(last[p.id], (p.cpuNs, at))
        }
        last = next
        stopping.formIntersection(next.keys)    // a stopped server that is gone is done stopping
        model.rows = rows(procs, cpu: cpu, stopping: stopping, home: userHome)
        if let u = usage(of: getpid()) {
            model.stats = trayStats(cpu: cpuPercent(mine, (u.cpu, at)), mem: u.mem)
            mine = (u.cpu, at)
        }
    }

    func stopServer(_ r: Row) {
        guard !stopping.contains(r.id) else { return }
        stopping.insert(r.id)
        model.error = nil
        sample()
        NSLog("stopping trello-mcp PID \(r.id.pid) (\(r.label))")
        stop(r.id, name: serverName) { [unowned self] fail in
            if let fail {
                stopping.remove(r.id)
                NSLog(fail)
                show(error: fail)
            }
            if window.isVisible { sample() }
        }
    }

    /// Shows a failure in the panel; reopens it if a click elsewhere already closed it, so it is seen once.
    func show(error: String) {
        if !window.isVisible { openPanel() }
        model.error = error
        NSAccessibility.post(element: NSApp as Any, notification: .announcementRequested,
                             userInfo: [.announcement: error, .priority: NSAccessibilityPriorityLevel.high.rawValue])
    }
}

let app = NSApplication.shared
let tray = Tray()
app.delegate = tray
app.setActivationPolicy(.accessory)
app.run()
