// trello-mcp-tray: a menu bar item whose panel lists the running trello-mcp servers with their
// CPU and memory and can stop one. It samples only while the panel shows (libproc, 0.5 s after
// opening, then every 2 s; the scan runs off the main thread) and sits idle otherwise. `make tray-install`
// starts it at login through ~/Library/LaunchAgents/com.trellomcp.tray.plist, which passes
// --cli <this checkout's bin/trello-mcp> and points stdout/stderr at ~/Library/Logs/trello-mcp-tray.log.
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

// KERN_PROCARGS2: argc, exec path, NUL padding, argv, then the environment.
let argc4 = withUnsafeBytes(of: Int32(4)) { Array($0) }
let buf = argc4 + Array("/My Apps/trello-mcp\0\0\0\0trello-mcp\0serve\0--config\0/a b/c.json\0HOME=/Users/me\0".utf8)
precondition(parseProcArgs(buf) == ["trello-mcp", "serve", "--config", "/a b/c.json"], "procargs: \(String(describing: parseProcArgs(buf)))")
precondition(parseProcArgs([1, 0]) == nil, "procargs: shorter than argc")
precondition(parseProcArgs(withUnsafeBytes(of: Int32(3)) { Array($0) } + Array("/x\0\0a\0".utf8)) == nil, "procargs: fewer strings than argc")
precondition(parseProcArgs(withUnsafeBytes(of: Int32(1)) { Array($0) } + Array("/x\0\0abc".utf8)) == nil, "procargs: cut before its NUL")

// The CLI path from the LaunchAgent: none by default, so "installed" never matches.
precondition(cliPath(["tray", "--cli", "/x/trello-mcp"]) == "/x/trello-mcp" && cliPath(["tray"]) == "" && cliPath(["tray", "--cli"]) == "", "--cli")

// Servers: scans, activity, groups.
let h = "/Users/me"
let t0 = Date(timeIntervalSince1970: 1_000_000)
let s0: UInt64 = 1_791_800_000_000_000     // a start time in µs
let ps0: UInt64 = 1_791_700_000_000_000    // a session's start time
func srv(_ pid: pid_t, cpu: UInt64 = 0, folder: String? = "/Users/me/work/app", ppid: pid_t = 900, parentStarted: UInt64? = ps0,
         start: UInt64 = s0, mem: UInt64 = 10 << 20, path: String = "/x/trello-mcp") -> ServerSample {
    ServerSample(id: ServerID(pid: pid, start: start), ppid: ppid, parent: "claude", parentStarted: parentStarted, folder: folder,
                 path: path, cpuNs: cpu, mem: mem)
}
precondition(activeCPU == 2_000_000, "active threshold")
let r1 = advance([], [srv(1)], prevAt: nil, at: t0)
precondition(r1 == [ServerRow(s: srv(1), cpu: nil, lastActive: nil)], "first scan: no cpu, no activity")
let r2 = advance(r1, [srv(1, cpu: 1_999_999)], prevAt: t0, at: t0 + 2)
precondition(r2[0].lastActive == nil && abs(r2[0].cpu! - 0.1) < 1e-6, "under 2 ms is idle: \(r2[0])")
let r3 = advance(r2, [srv(1, cpu: 4_000_000)], prevAt: t0 + 2, at: t0 + 4)
precondition(r3[0].lastActive == t0 + 4 && abs(r3[0].cpu! - 0.1) < 1e-6, "2 ms or more is activity: \(r3[0])")
let r4 = advance(r3, [srv(1, cpu: 4_000_000)], prevAt: t0 + 4, at: t0 + 64)
precondition(r4[0].lastActive == t0 + 4 && r4[0].cpu == 0, "activity is remembered")
let r5 = advance(r4, [srv(1, cpu: 4_000_000)], prevAt: nil, at: t0 + 9000)
precondition(r5[0].lastActive == t0 + 4 && r5[0].cpu == nil, "a panel reopened: no CPU from a stale baseline, activity kept")
let reused = advance(r4, [srv(1, cpu: 9_000_000, start: s0 + 1)], prevAt: t0 + 64, at: t0 + 66)
precondition(reused[0].lastActive == nil && reused[0].cpu == nil, "a reused pid starts fresh")
precondition(advance(r4, [], prevAt: t0 + 64, at: t0 + 66).isEmpty, "gone servers drop out")
precondition(advance(r4, [srv(1, cpu: 1)], prevAt: t0 + 64, at: t0 + 66)[0].cpu == 0, "a lower CPU total reads 0")
precondition(advance(r1, [srv(1, cpu: 5)], prevAt: t0, at: t0)[0].cpu == nil, "no time between scans: no cpu")
precondition(sameShape(r3, r4) && !sameShape(r4, reused) && !sameShape(r4, []), "shape: same ids")
precondition(!sameShape(r4, advance(r4, [srv(1, folder: "/elsewhere")], prevAt: t0 + 64, at: t0 + 66)), "shape: moved folder")
precondition(sameShape(advance([], [srv(1), srv(2)], prevAt: nil, at: t0), advance([], [srv(2), srv(1)], prevAt: nil, at: t0)), "shape: order does not matter")
precondition(activeText(nil, now: t0) == "—" && activeText(t0 - 59, now: t0) == "now", "active: none, now")
precondition(activeText(t0 - 60, now: t0) == "1m ago" && activeText(t0 - 3599, now: t0) == "59m ago", "active: minutes")
precondition(activeText(t0 - 3600, now: t0) == "1h ago" && activeText(t0 - 86400 * 2, now: t0) == "2d ago", "active: hours, days")
// activeText across two scans, as the panel shows it.
let a2 = advance(advance([], [srv(1)], prevAt: nil, at: t0), [srv(1, cpu: 3_000_000)], prevAt: t0, at: t0 + 2)
precondition(activeText(a2[0].lastActive, now: t0 + 2) == "now" && activeText(a2[0].lastActive, now: t0 + 12 * 60 + 2) == "12m ago", "active text after a busy scan")
precondition(countdown(6 * 86400 + 21 * 3600 + 300) == "6d 21h" && countdown(5 * 3600 + 12 * 60 + 59) == "5h 12m", "countdown: days, hours")
precondition(countdown(12 * 60 + 30) == "12m" && countdown(-5) == "0m", "countdown: minutes, clock skew")
let sessionStart = Date(timeIntervalSince1970: Double(ps0) / 1e6)
precondition(upText(r1[0], now: sessionStart + 2 * 86400 + 9 * 3600) == "2d 9h", "up: session age")
precondition(upText(advance([], [srv(2, folder: nil, ppid: 1, parentStarted: nil)], prevAt: nil, at: t0)[0], now: t0) == "—", "up: no session")
precondition(buildLabel("/Users/me/p/trello-mcp/bin/trello-mcp", cli: "/Users/me/p/trello-mcp/bin/trello-mcp") == "installed", "build: installed")
precondition(buildLabel("/r/trello-mcp/.claude/worktrees/feat-x/bin/trello-mcp", cli: "/c") == "feat-x", "build: worktree")
precondition(buildLabel("/Users/me/old/bin/trello-mcp", cli: "/c") == "old/bin" && buildLabel("trello-mcp", cli: "/c") == "trello-mcp", "build: other")
precondition(buildLabel("/Users/me/old/bin/trello-mcp", cli: "") == "old/bin", "build: no --cli never matches")
precondition(groupLabel("/Users/me/Projects/eventcreate/eventcreate_app", home: h) == "eventcreate/eventcreate_app", "group: two folders")
precondition(groupLabel("/Users/me", home: h) == "~" && groupLabel("/", home: h) == "/" && groupLabel("/tmp", home: h) == "tmp", "group: home, root")
precondition(groupLabel(nil, home: h) == noSession && groupLabel("", home: h) == noSession, "group: none")
let mix = advance([], [
    srv(5, folder: "/a/zeta"), srv(3, folder: nil, ppid: 1, parentStarted: nil), srv(7, folder: nil, ppid: 880, parentStarted: nil),
    srv(4, folder: "/a/Alpha", ppid: 901, parentStarted: 300), srv(2, folder: "/a/Alpha", ppid: 902, parentStarted: 100),
    srv(6, folder: "/a/Alpha", ppid: 902, parentStarted: 100, mem: 3 << 20), srv(8, folder: h, ppid: 903, parentStarted: 50),
], prevAt: nil, at: t0)
let gs = groups(mix, home: h)
precondition(gs.map(\.label) == ["a/Alpha", "a/zeta", "~", noSession], "groups: alphabetical (~ after letters), no session last: \(gs.map(\.label))")
precondition(gs[0].rows.map(\.s.id.pid) == [2, 6, 4], "rows: oldest session first, then pid")
precondition(gs[3].rows.map(\.s.id.pid) == [3, 7], "no session: a launchd child and an orphan of a gone parent, by start then pid")
precondition(gs.map(groupMeta) == ["2 sessions · 23 MB", "1 session · 10 MB", "1 session · 10 MB", "20 MB"], "group meta: \(gs.map(groupMeta))")
precondition(serverFigures(mix) == ["7", "4", "63 MB", "—"] && serverFigures(r4) == ["1", "1", "10 MB", "0.0%"], "figures: \(serverFigures(mix)) \(serverFigures(r4))")
precondition(abs(totalCPU(advance(r1, [srv(1, cpu: 500_000_000)], prevAt: t0, at: t0 + 2))! - 25) < 1e-9 && totalCPU([]) == nil, "total cpu")
precondition(trayStats(nil, cpu: 5, mem: 21 << 20, at: t0) == "Tray: CPU — · Memory 21 MB", "tray stats: first sample")
precondition(trayStats((1_000_000_000, t0), cpu: 1_020_000_000, mem: 21 << 20, at: t0 + 2) == "Tray: CPU 1.0% · Memory 21 MB", "tray stats: 20 ms over 2 s")

// Text.
precondition(memText(8_598_323) == "8.2 MB" && memText(49 << 20) == "49 MB" && memText(1229 << 20) == "1.2 GB", "mem: \(memText(8_598_323))")
precondition(cpuText(nil) == "—" && cpuText(0.04) == "0.0%" && cpuText(96.84) == "96.8%" && cpuText(143.2) == "143%", "cpu text")
precondition(stopFailure(4648, EPERM) == "Could not stop PID 4648: Operation not permitted" && stopFailure(4648, ESRCH) == nil, "stop failure text")
precondition(stopGrace == 5, "SIGKILL after 5 s")

// The Confirm flow: the first click arms, 3 s disarm it, the second click stops.
let cm = PanelModel()
var stops: [ServerID] = []
cm.onStop = { stops.append($0) }
let qa = ServerID(pid: 42, start: 1), qb = ServerID(pid: 43, start: 1)
cm.quitClicked(qa)
precondition(cm.confirming == qa && stops.isEmpty, "first click arms Confirm")
cm.quitClicked(qb)
precondition(cm.confirming == qb && stops.isEmpty, "a click on another row re-arms that row")
cm.quitClicked(qb)
precondition(cm.confirming == nil && stops == [qb], "second click stops")
cm.quitClicked(qa); RunLoop.main.run(until: Date() + 1)
cm.quitClicked(qb); RunLoop.main.run(until: Date() + 1)
cm.quitClicked(qa); RunLoop.main.run(until: Date() + 1.5)
precondition(cm.confirming == qa, "a stale timer must not end a newer Confirm")
RunLoop.main.run(until: Date() + 2.0)
precondition(cm.confirming == nil && stops == [qb], "Confirm lapses after 3 s without stopping")
cm.quitClicked(qa); cm.quitClicked(qa)
precondition(stops == [qb, qa] && cm.confirming == nil, "a second click after a lapse starts over")
let sp = pal(dark: false)
precondition(buttonColors(.confirm, hot: false, p: sp).bg == swissRed, "confirm draws the Swiss red fill")
precondition(buttonColors(.quit, hot: true, p: sp).bg == sp.ink && buttonColors(.quit, hot: false, p: sp).bg == nil, "hot Quit draws the ink fill")
precondition(buttonColors(.stopping, hot: true, p: sp).bg == nil, "Quitting is never hot-filled")
var pressed = 0
let sl = ServerLine(["1", "b", "1m", "1m", "0%", "1 MB"], sp, path: "/x", action: .quit) { pressed += 1 }
sl.pointerMoved(inside: true)
precondition(sl.hot, "entering the button turns it hot")
sl.action = .confirm
precondition(!sl.hot && sl.title == "Confirm", "a click leaves the button cold, so Confirm draws red")
sl.pointerMoved(inside: true)
precondition(!sl.hot, "a move inside the button after the click keeps Confirm red")
sl.pointerMoved(inside: false)
sl.pointerMoved(inside: true)
precondition(sl.hot, "leaving and coming back turns it hot")
sl.action = .stopping
precondition(sl.title == "Quitting…" && (sl.accessibilityChildren()?.first as? NSAccessibilityElement)?.isAccessibilityEnabled() == false, "Quitting… is disabled")

// libproc on this process.
guard let me = bsdInfo(getpid()) else { fatalError("own bsd info") }
precondition(me.ppid == getppid() && me.start > 0, "bsd info")
precondition(procName(getpid())?.hasPrefix("trello-mcp-tray") == true, "own name: \(String(describing: procName(getpid())))")
precondition(folder(of: getpid()) == FileManager.default.currentDirectoryPath, "own folder: \(String(describing: folder(of: getpid())))")
precondition(args(of: getpid()) == CommandLine.arguments, "own args")
precondition(userPids().contains(getpid()), "own pid is listed")
guard let u0 = usage(of: getpid()) else { fatalError("own usage reads") }
// Spin until 0.3 s of CPU is charged, not 0.3 s of wall time: a loaded Mac only makes this take longer.
let c0 = clock_gettime_nsec_np(CLOCK_PROCESS_CPUTIME_ID)
var n = 0
while clock_gettime_nsec_np(CLOCK_PROCESS_CPUTIME_ID) - c0 < 300_000_000 { n &+= 1 }
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
var found: ServerSample?
for _ in 0..<100 where found == nil {
    found = sc.scan().first { $0.id.pid == child.processIdentifier }
    if found == nil { usleep(10_000) }
}
guard let f = found else { fatalError("the fake server is found") }
precondition(f.path.hasSuffix("/tray selftest \(getpid())/trello-mcp"), "binary path with a space: \(f)")
precondition(f.ppid == getpid() && f.parent.hasPrefix("trello-mcp-tray") && f.mem > 0, "parent, memory: \(f)")
precondition(f.parentStarted == bsdInfo(getpid())?.start && f.folder == FileManager.default.currentDirectoryPath,
             "its session is this process: start and folder are the parent's, not the server's (\(dir.path)): \(f)")
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

// Menu bar icon and panel rendering.
precondition(boardImage().isTemplate && boardImage().size == NSSize(width: 16, height: 16), "menu bar icon: \(boardImage().size)")
_ = NSApplication.shared
let pm = PanelModel(), pc = PanelController(pm)
pm.now = { t0 + 3600 }
pm.cli = "/Users/me/p/trello-mcp/bin/trello-mcp"
func panelSize() -> NSSize { pc.render(); return pc.view.subviews.first?.fittingSize ?? .zero }
func views(_ v: NSView) -> [NSView] { [v] + v.subviews.flatMap(views) }
pm.stats = trayStats(nil, cpu: 5, mem: 21 << 20, at: t0)
let shown = advance([], [srv(10, folder: "/Users/me/P/eventcreate_app", path: "/Users/me/p/trello-mcp/bin/trello-mcp"), srv(11, folder: "/Users/me/P/eventcreate_app", ppid: 901),
                         srv(12, folder: nil, ppid: 1, parentStarted: nil)], prevAt: nil, at: t0)
pm.rows = shown
pm.error = stopFailure(20, EPERM)
precondition(panelSize().width == 520, "panel with servers and an error: \(panelSize())")
pm.confirming = shown[0].s.id
pm.stopping = [shown[1].s.id]
precondition(panelSize().width == 520, "a Confirm row and a Quitting… row")
let wide = srv(99999, cpu: 0, folder: "/Users/me/" + String(repeating: "very_long_project_name_", count: 3), mem: 1229 << 20, path: "/" + String(repeating: "long_dir/", count: 20) + "trello-mcp")
pm.rows = advance(advance([], [wide], prevAt: nil, at: t0), [srv(99999, cpu: 14_320_000_000, folder: wide.folder, mem: 1229 << 20)], prevAt: t0, at: t0 + 100)
precondition(panelSize().width == 520, "long label, 143% and 1.2 GB keep the width: \(panelSize())")
pm.rows = advance([], (1...40).map { i in srv(pid_t(1000 + i), folder: "/Users/me/P/project\(i)", ppid: pid_t(2000 + i), start: s0 + UInt64(i)) }, prevAt: nil, at: t0)
pm.cap = 400
precondition(panelSize().height < 400 + 220 && panelSize().width == 520, "40 rows scroll inside the cap: \(panelSize())")
// A re-render keeps the scroll offset while the panel shows (hosted in a real window, off screen, so layout runs).
let host = NSWindow(contentRect: NSRect(x: -10000, y: 0, width: 520, height: 800), styleMask: [.borderless], backing: .buffered, defer: false)
host.contentViewController = pc
host.orderFront(nil)
func clip() -> NSClipView { ((pc.view.subviews.first as! NSStackView).arrangedSubviews[1] as! NSScrollView).contentView }
pc.render()
precondition(clip().documentView!.frame.width <= clip().bounds.width, "rows never scroll sideways: doc \(clip().documentView!.frame.width), clip \(clip().bounds.width)")
clip().scroll(to: NSPoint(x: 0, y: 500))
clip().superview.map { ($0 as! NSScrollView).reflectScrolledClipView(clip()) }
pc.render()
precondition(clip().bounds.origin.y == 500, "scroll offset kept: \(clip().bounds.origin.y)")
// A tick (same servers, new numbers) updates the rows in place: same views, new text, scroll kept.
let before = views(pc.view).compactMap { $0 as? ServerLine }
pm.rows = advance(pm.rows, pm.rows.map { r in srv(r.s.id.pid, cpu: 50_000_000, folder: r.s.folder, ppid: r.s.ppid, start: r.s.id.start) }, prevAt: t0, at: t0 + 2)
pm.confirming = pm.rows[3].s.id
pm.now = { t0 + 3 }
let same = pm.rows
pm.rows = same
let after = views(pc.view).compactMap { $0 as? ServerLine }
precondition(before.count == 40 && zip(before, after).allSatisfy { $0 === $1 }, "a tick keeps the row views")
precondition(after[0].values[4] == "2.5%" && after[0].values[3] == "now" && after.filter { $0.title == "Confirm" }.map { $0.values[0] } == ["\(pm.rows[3].s.id.pid)"] && after.filter { $0.title == "Quit" }.count == 39, "a tick updates values and buttons: \(after[0].values)")
precondition(clip().bounds.origin.y == 500, "a tick keeps the scroll offset")
pm.rows = Array(pm.rows.prefix(8))
pc.render()
let docH = clip().documentView!.frame.height
precondition(docH - 400 < 500 && clip().bounds.origin.y == docH - 400, "offset clamped to the shorter list: \(clip().bounds.origin.y), doc \(docH)")
host.orderOut(nil)
pc.render()
precondition(clip().bounds.origin.y == 0, "a hidden panel reopens at the top")
host.contentViewController = nil
pm.rows = []
pm.error = nil
precondition(panelSize().width == 520, "empty state")
pm.error = "Could not stop PID 4648: Operation not permitted"
precondition(panelSize().width == 520, "empty state with an error")
pm.error = nil
pc.view.appearance = NSAppearance(named: .darkAqua)
pm.rows = shown
precondition(panelSize().width == 520, "dark")
pc.view.appearance = NSAppearance(named: .aqua)
precondition(panelSize().width == 520, "light")

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
    let scanner = Scanner()                 // not thread-safe: only scanQueue touches it, one scan at a time
    let scanQueue = DispatchQueue(label: "trello-mcp-tray.scan", qos: .utility)
    var scanning = false                    // a scan is in flight (main thread only)
    var afterScan: [() -> Void] = []        // run when the scan in flight lands
    var scannedAt: Date?                    // the previous scan; nil after the panel opens: a closed panel has no baseline
    var traySample: (cpu: UInt64, at: Date)?    // the tray's own previous sample
    var opening = false                     // the first scan of an open is in flight
    var clicks: Any?            // watches clicks in other apps while the panel shows
    var timer: Timer?           // the next scan, only while the panel shows

    func applicationDidFinishLaunching(_ n: Notification) {
        item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = boardImage()
        item.button?.toolTip = "Trello MCP"
        item.button?.setAccessibilityLabel("Trello MCP")
        item.button?.target = self
        item.button?.action = #selector(togglePanel)
        model.cli = cliPath(CommandLine.arguments)
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
        guard !opening, let b = item.button, let bar = b.window else { return }
        let r = bar.convertToScreen(b.convert(b.bounds, to: nil)), vis = (bar.screen ?? NSScreen.main)?.visibleFrame ?? .zero
        model.cap = bar.frame.minY - vis.minY - 160    // room for header and footer; the bar's bottom edge, since an auto-hidden menu bar is inside visibleFrame
        scannedAt = nil                 // CPU since the panel last closed would be a stale average
        traySample = nil
        opening = true
        scan { [unowned self] in        // the panel appears with the first scan in it, not empty
            opening = false
            panel.render()              // sizes the window
            let s = window.frame.size
            window.setFrameOrigin(NSPoint(x: min(max(r.midX - s.width / 2, vis.minX), vis.maxX - s.width), y: bar.frame.minY - s.height))
            window.makeKeyAndOrderFront(nil)
            // The status item's own clicks reach us as global events; its action does the toggling, so clicks anywhere in the status bar window are left alone.
            clicks = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [unowned self] e in
                if !bar.frame.contains(e.locationInWindow) { closePanel() }
            }
            scanAgain(after: 0.5)       // CPU needs a second scan: show it almost at once
        }
    }

    /// Scans after `delay`, then every 2 s while the panel shows. The tolerance lets macOS batch wakeups.
    func scanAgain(after delay: TimeInterval) {
        let t = Timer(timeInterval: delay, repeats: false) { [unowned self] _ in
            scan()
            scanAgain(after: 2)
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
        model.confirming = nil
    }

    /// Reads the servers off the main thread, one scan at a time (a tick that finds one running is
    /// skipped), and applies the result on main. `then` runs after the scan that lands next.
    func scan(then: (() -> Void)? = nil) {
        if let then { afterScan.append(then) }
        guard !scanning else { return }
        scanning = true
        scanQueue.async { [unowned self] in
            let found = scanner.scan()
            let at = Date()
            DispatchQueue.main.async { [unowned self] in
                scanning = false
                apply(found, at: at)
                let waiting = afterScan
                afterScan = []
                waiting.forEach { $0() }
            }
        }
    }

    /// Hands a scan and the tray's own usage to the panel.
    func apply(_ found: [ServerSample], at: Date) {
        let live = model.stopping.intersection(found.map(\.id))    // a stopped server that is gone is done stopping
        if live != model.stopping { model.stopping = live }
        if let u = usage(of: getpid()) {
            model.stats = trayStats(traySample, cpu: u.cpu, mem: u.mem, at: at)
            traySample = (u.cpu, at)
        }
        model.rows = advance(model.rows, found, prevAt: scannedAt, at: at)
        scannedAt = at
    }

    /// Quit's Confirm: SIGTERM, SIGKILL after 5 s; the row says Quitting… until then.
    func stopServer(_ id: ServerID) {
        guard !model.stopping.contains(id) else { return }
        let r = model.rows.first { $0.id == id }
        NSLog("stopping trello-mcp PID \(id.pid) (\(r?.s.path ?? "?")) of \(r?.s.folder ?? "no session")")
        model.stopping.insert(id)
        model.error = nil
        stop(id, name: serverName) { [unowned self] fail in
            model.stopping.remove(id)
            if let fail {
                NSLog(fail)
                show(error: fail)
            }
            if window.isVisible { scan() }
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
