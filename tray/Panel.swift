// The tray's panel in plain AppKit, after claude-usage's tray and its DESIGN.md (the Swiss
// instrument sheet): Helvetica Neue, ink on paper, rules not boxes, Swiss red only for alarms.
// AppKit, not SwiftUI, keeps that framework out of a menu bar process.
import AppKit

// MARK: - Model

/// The panel's state. A change of shape re-renders the panel while it shows; a tick (same servers,
/// new numbers) only updates text in place, which keeps hover and scroll.
final class PanelModel {
    var rows: [ServerRow] = [] { didSet { sameShape(oldValue, rows) ? ticked() : changed() } }
    var confirming: ServerID? { didSet { ticked() } }   // Quit clicked once: its Confirm shows
    var stopping: Set<ServerID> = [] { didSet { ticked() } }
    var error: String? { didSet { changed() } }
    var cap: CGFloat = 800 { didSet { changed() } }     // tallest the scrolling middle may be; Tray sets it from the screen
    var stats = "" { didSet { ticked() } }               // the tray's own CPU and memory
    var cli = ""                                         // the tray's CLI path, for Build's "installed"
    var now: () -> Date = Date.init
    var changed: () -> Void = {}                         // set by PanelController
    var ticked: () -> Void = {}                          // set by PanelController
    // Set by Tray.
    var onStop: (ServerID) -> Void = { _ in }
    var onLog: () -> Void = {}
    var onQuit: () -> Void = {}

    private var confirmTimer: DispatchWorkItem?

    /// The first click on Quit arms Confirm for 3 s; a click on Confirm stops the server.
    func quitClicked(_ id: ServerID) {
        guard confirming == id else {
            confirming = id
            confirmTimer?.cancel()      // an older timer must not end this Confirm early
            let t = DispatchWorkItem { [weak self] in
                if self?.confirming == id { self?.confirming = nil }
            }
            confirmTimer = t
            DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: t)
            return
        }
        confirmTimer?.cancel()
        confirmTimer = nil
        confirming = nil
        onStop(id)
    }
}

// MARK: - Palette

struct Pal {
    let paper, ink, muted, hair, track, redInk: NSColor
}

extension NSColor {
    convenience init(hex: UInt32) { self.init(srgbRed: CGFloat(hex >> 16 & 255) / 255, green: CGFloat(hex >> 8 & 255) / 255, blue: CGFloat(hex & 255) / 255, alpha: 1) }
}

let swissRed = NSColor(hex: 0xda291c)

/// DESIGN.md values.
func pal(dark: Bool) -> Pal {
    dark ? Pal(paper: NSColor(hex: 0x0d0d0d), ink: NSColor(hex: 0xf2f2f2), muted: NSColor(hex: 0xa3a3a3), hair: NSColor(hex: 0x333333), track: NSColor(hex: 0x2a2a2a), redInk: NSColor(hex: 0xff6a5c))
        : Pal(paper: NSColor(hex: 0xffffff), ink: NSColor(hex: 0x111111), muted: NSColor(hex: 0x5c5c5c), hair: NSColor(hex: 0xd4d4d4), track: NSColor(hex: 0xe6e6e6), redInk: NSColor(hex: 0xda291c))
}

// MARK: - Pieces

enum W { case r, m, b }

private var fonts: [String: NSFont] = [:]

/// Helvetica Neue at a fixed size, with tabular digits. Cached: each tick draws a few hundred strings.
func hvFont(_ size: CGFloat, _ w: W = .r) -> NSFont {
    let key = "\(size) \(w)"
    if let f = fonts[key] { return f }
    let f = NSFont(name: w == .b ? "HelveticaNeue-Bold" : w == .m ? "HelveticaNeue-Medium" : "HelveticaNeue", size: size) ?? .systemFont(ofSize: size)
    let tnum: [NSFontDescriptor.FeatureKey: Int] = [.typeIdentifier: kNumberSpacingType, .selectorIdentifier: kMonospacedNumbersSelector]
    let out = NSFont(descriptor: f.fontDescriptor.addingAttributes([.featureSettings: [tnum]]), size: size) ?? f
    fonts[key] = out
    return out
}

/// A text label. `em` is letter-spacing in em; `wrap` is the width it wraps at, else it stays on one line, cut per `cut`.
func label(_ s: String, _ size: CGFloat, _ w: W = .r, em: CGFloat = 0, _ color: NSColor, wrap: CGFloat? = nil,
           align: NSTextAlignment = .left, cut: NSLineBreakMode = .byTruncatingTail) -> NSTextField {
    let ps = NSMutableParagraphStyle()
    ps.alignment = align
    ps.lineBreakMode = wrap == nil ? cut : .byWordWrapping
    let a = NSAttributedString(string: s, attributes: [.font: hvFont(size, w), .kern: em * size, .foregroundColor: color, .paragraphStyle: ps])
    guard let wrap else { return NSTextField(labelWithAttributedString: a) }
    let f = NSTextField(wrappingLabelWithString: "")
    f.isSelectable = false
    f.attributedStringValue = a
    f.preferredMaxLayoutWidth = wrap
    return f
}

/// A regular-weight label.
func label(_ s: String, _ size: CGFloat, _ color: NSColor, wrap: CGFloat? = nil, align: NSTextAlignment = .left) -> NSTextField {
    label(s, size, .r, color, wrap: wrap, align: align)
}

func recolor(_ f: NSTextField, _ c: NSColor) {
    let a = NSMutableAttributedString(attributedString: f.attributedStringValue)
    a.addAttribute(.foregroundColor, value: c, range: NSRange(location: 0, length: a.length))
    f.attributedStringValue = a
}

/// Replaces a label's text and color, keeping its font, kerning and paragraph style; skips a no-op.
/// Labels are never empty, so the first character's attributes carry over.
func setText(_ f: NSTextField, _ s: String, _ c: NSColor) {
    let old = f.attributedStringValue
    guard old.length > 0, old.string != s || old.attribute(.foregroundColor, at: 0, effectiveRange: nil) as? NSColor != c else { return }
    let a = NSMutableAttributedString(attributedString: old)
    a.replaceCharacters(in: NSRange(location: 0, length: a.length), with: s)
    a.addAttribute(.foregroundColor, value: c, range: NSRange(location: 0, length: a.length))
    f.attributedStringValue = a
}

/// Flipped, so a scroll view shows it from the top.
final class Stack: NSStackView {
    override var isFlipped: Bool { true }
}

/// A column; each item stretches to its width less the insets.
func vstack(_ items: [NSView], _ spacing: CGFloat = 0, inset: NSEdgeInsets = NSEdgeInsetsZero) -> Stack {
    let s = Stack(views: items)
    s.orientation = .vertical
    s.alignment = .leading
    s.spacing = spacing
    s.edgeInsets = inset
    for v in items { v.widthAnchor.constraint(equalTo: s.widthAnchor, constant: -inset.left - inset.right).isActive = true }
    return s
}

/// A row, centered or top-aligned; a `spacer()` or a `stretch`ed item takes the room left.
func hstack(_ items: [NSView], _ spacing: CGFloat = 0, top: Bool = false) -> Stack {
    let s = Stack(views: items)
    s.orientation = .horizontal
    s.alignment = top ? .top : .centerY
    s.distribution = .fill
    s.spacing = spacing
    return s
}

@discardableResult
func stretch<V: NSView>(_ v: V) -> V {
    v.setContentHuggingPriority(.init(1), for: .horizontal)
    return v
}

func spacer() -> NSView { stretch(NSView()) }

@discardableResult
func fixed<V: NSView>(_ v: V, w: CGFloat? = nil, h: CGFloat? = nil) -> V {
    v.translatesAutoresizingMaskIntoConstraints = false
    if let w { v.widthAnchor.constraint(equalToConstant: w).isActive = true }
    if let h { v.heightAnchor.constraint(equalToConstant: h).isActive = true }
    return v
}

/// A plain block of color: rules, squares.
func block(_ c: NSColor, w: CGFloat? = nil, h: CGFloat? = nil) -> NSView {
    let v = NSView()
    v.wantsLayer = true
    v.layer?.backgroundColor = c.cgColor
    return fixed(v, w: w, h: h)
}

/// `v` inside `inset`.
func pad(_ v: NSView, _ inset: NSEdgeInsets) -> Stack { vstack([v], inset: inset) }

func edges(_ t: CGFloat, _ l: CGFloat, _ b: CGFloat, _ r: CGFloat) -> NSEdgeInsets { NSEdgeInsets(top: t, left: l, bottom: b, right: r) }

/// The board mark: three columns of different heights hanging from the top, like lists on a board.
let boardColumns: [CGFloat] = [1, 0.65, 0.8]

func boardMark(_ h: CGFloat, _ c: NSColor) -> NSView {
    hstack(boardColumns.map { block(c, w: h / 4, h: h * $0) }, h / 8, top: true)
}

/// A borderless button made of its own views; `paint` restyles it when it turns hot (hover or press) or back.
/// The focus ring is the system's.
final class Press: NSButton {
    var paint: ((Press, Bool) -> Void)? { didSet { repaint() } }
    override var isEnabled: Bool { didSet { repaint() } }
    private let content: NSView, pad: (CGFloat, CGFloat), minHeight: CGFloat, act: () -> Void
    private var hover = false, down = false
    private lazy var fit = content.fittingSize

    /// `pad` is (leading, trailing); `content` sits in the middle.
    init(_ content: NSView, pad: (CGFloat, CGFloat) = (0, 0), minHeight: CGFloat = 0, _ act: @escaping () -> Void) {
        self.content = content
        self.pad = pad
        self.minHeight = minHeight
        self.act = act
        super.init(frame: .zero)
        title = ""
        isBordered = false
        wantsLayer = true
        target = self
        action = #selector(fire)
        translatesAutoresizingMaskIntoConstraints = false
        content.translatesAutoresizingMaskIntoConstraints = false
        addSubview(content)
        NSLayoutConstraint.activate([
            content.centerYAnchor.constraint(equalTo: centerYAnchor),
            content.topAnchor.constraint(greaterThanOrEqualTo: topAnchor),
            content.centerXAnchor.constraint(equalTo: centerXAnchor, constant: (pad.0 - pad.1) / 2),
            content.leadingAnchor.constraint(greaterThanOrEqualTo: leadingAnchor, constant: pad.0),
        ])
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: NSSize { NSSize(width: fit.width + pad.0 + pad.1, height: max(minHeight, fit.height)) }
    override var focusRingMaskBounds: NSRect { bounds }
    override func drawFocusRingMask() { bounds.fill() }
    override func accessibilityChildren() -> [Any]? { nil }
    override func mouseEntered(with e: NSEvent) { hover = true; repaint() }
    override func mouseExited(with e: NSEvent) { hover = false; repaint() }

    override func mouseDown(with e: NSEvent) {
        down = true
        repaint()
        super.mouseDown(with: e)    // tracks until the mouse is up and fires the action
        down = false
        repaint()
    }

    @objc private func fire() { act() }
    private func repaint() { paint?(self, isEnabled && (hover || down)) }
}

/// A text-only button (the design's .txt): red ink while hot.
func txtBtn(_ p: Pal, _ title: String, _ w: W, color: NSColor? = nil, pad: (CGFloat, CGFloat), key: String = "", _ act: @escaping () -> Void) -> Press {
    let l = label(title, 13, w, color ?? p.ink)
    let b = Press(l, pad: pad, minHeight: 36, act)
    b.paint = { _, hot in recolor(l, hot ? p.redInk : color ?? p.ink) }
    b.keyEquivalent = key
    b.keyEquivalentModifierMask = .command
    b.setAccessibilityLabel(title)
    return b
}


// MARK: - Panel

let panelWidth: CGFloat = 520
let inner: CGFloat = panelWidth - 32     // the rows' width: the panel less its 16 pt margins

/// The panel's content: rebuilt from the model on each change of shape while it shows.
final class PanelController: NSViewController {
    let m: PanelModel
    private var queued = false
    // What a tick updates in place; filled by render().
    private var lines: [ServerID: ServerLine] = [:]
    private var groupMetas: [String: GroupLine] = [:]
    private var figures: [NSTextField] = []     // servers, sessions, memory, CPU
    private var statsRef: NSTextField?
    private var scrollRef: NSScrollView?
    private var p = pal(dark: false)
    private let home = userHome

    init(_ m: PanelModel) {
        self.m = m
        super.init(nibName: nil, bundle: nil)
        m.changed = { [unowned self] in
            guard !queued else { return }
            queued = true
            DispatchQueue.main.async { [unowned self] in
                if queued, view.window?.isVisible == true { render() }
                queued = false
            }
        }
        m.ticked = { [unowned self] in if !queued { tick() } }    // a queued render covers the tick
    }

    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        let r = Root()
        r.wantsLayer = true
        r.onAppearance = { [unowned self] in render() }
        view = r
    }

    func render() {
        queued = false      // this render covers changes still queued
        let dark = view.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        p = pal(dark: dark)
        lines = [:]
        groupMetas = [:]
        figures = []
        let keptY = view.window?.isVisible == true ? scrollRef?.contentView.bounds.origin.y ?? 0 : 0     // a re-render keeps the scroll position
        // Header and footer stay put; the middle scrolls once it outgrows the screen.
        // The scroll view runs into the right margin, so its thin overlay scroller sits there and never covers or narrows the rows.
        let mid = vstack(middle(), 16, inset: edges(0, 0, 0, 16))
        mid.widthAnchor.constraint(equalToConstant: panelWidth - 16).isActive = true
        let h = mid.fittingSize.height
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = h > m.cap
        scroll.scrollerStyle = .overlay     // a legacy scroller would take width from the clip view and make the rows scroll sideways
        scroll.verticalScroller?.controlSize = .small
        scroll.verticalScrollElasticity = h > m.cap ? .automatic : .none
        scroll.horizontalScrollElasticity = .none
        scroll.documentView = mid
        scrollRef = scroll
        NSLayoutConstraint.activate([
            mid.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            mid.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            scroll.heightAnchor.constraint(equalToConstant: min(h, m.cap)),
        ])
        let col = vstack([pad(header(), edges(0, 0, 0, 16)), scroll, pad(footer(), edges(0, 0, 0, 16))], 16, inset: edges(16, 16, 16, 0))
        col.translatesAutoresizingMaskIntoConstraints = false
        view.subviews.forEach { $0.removeFromSuperview() }
        view.addSubview(col)
        NSLayoutConstraint.activate([
            col.widthAnchor.constraint(equalToConstant: panelWidth),
            col.topAnchor.constraint(equalTo: view.topAnchor),
            col.leadingAnchor.constraint(equalTo: view.leadingAnchor),
        ])
        view.layer?.backgroundColor = p.paper.cgColor
        view.layoutSubtreeIfNeeded()    // the clip view has no height before layout, and would clamp the offset to 0
        scroll.contentView.scroll(to: NSPoint(x: 0, y: min(keptY, max(0, h - m.cap))))
        scroll.reflectScrolledClipView(scroll.contentView)
        let size = col.fittingSize
        if let w = view.window {    // keep the top edge, under the status item
            w.setFrame(NSRect(x: w.frame.minX, y: w.frame.maxY - size.height, width: size.width, height: size.height), display: true)
        }
    }

    /// New numbers for the same rows: values, buttons, group metas, figures and stats in place.
    func tick() {
        guard view.window?.isVisible == true else { return }
        let now = m.now()
        for r in m.rows {
            guard let l = lines[r.id] else { continue }
            l.values = serverCells(r, now: now)
            l.action = action(r.id)
        }
        for g in groups(m.rows, home: home) { groupMetas[g.label]?.meta = groupMeta(g) }
        zip(figures, serverFigures(m.rows)).forEach { setText($0, $1, p.ink) }
        if let s = statsRef, !m.stats.isEmpty { setText(s, m.stats, p.muted) }
    }

    func middle() -> [NSView] {
        var out: [NSView] = []
        if let e = m.error {
            let box = pad(label(e, 13, .b, p.redInk, wrap: inner - 20), edges(10, 10, 10, 10))
            box.wantsLayer = true
            box.layer?.borderWidth = 2
            box.layer?.borderColor = p.redInk.cgColor
            out.append(box)
        }
        return out + [m.rows.isEmpty ? empty() : servers()]
    }

    func header() -> NSView {
        let top = hstack([hstack([boardMark(22, p.ink), label("Trello MCP", 15, .b, em: -0.02, p.ink)], 8), spacer(),
                          label("live · every 2 s", 12, p.muted)], 12)
        return vstack([top, block(p.ink, h: 4)], 12)
    }

    func footer() -> NSView {
        let row = hstack([
            txtBtn(p, "Open Log", .b, pad: (0, 8), key: "l") { [unowned self] in m.onLog() },
            spacer(),
            txtBtn(p, "Quit", .r, color: p.muted, pad: (8, 0), key: "q") { [unowned self] in m.onQuit() },
        ], 4)
        let s = label(m.stats.isEmpty ? "Tray: —" : m.stats, 12, p.muted)
        statsRef = s
        return vstack([block(p.ink, h: 1), row, s], 3)
    }

    func caps(_ s: String, align: NSTextAlignment = .left) -> NSTextField { label(s, 11, .m, em: 0.06, p.muted, align: align) }

    // MARK: Servers

    /// Figures (servers, sessions, memory, CPU), then the table: PID, build, up, active, CPU, memory and the button, grouped by session folder.
    func servers() -> NSView {
        let title = vstack([caps("TRELLO-MCP SERVERS"), block(p.ink, h: 1)], 5)
        let now = m.now()
        let figs = hstack(zip(["SERVERS", "SESSIONS", "MEMORY", "CPU"], serverFigures(m.rows)).map { n, v in
            let f = label(v, 24, .b, em: -0.02, p.ink)
            figures.append(f)
            return vstack([caps(n), f], 2)
        } + [spacer()], 20, top: true)
        let head = line(caps("PID", align: .right), caps("BUILD"), caps("UP", align: .right), caps("ACTIVE", align: .right),
                        caps("CPU", align: .right), caps("MEM", align: .right), NSView())
        let rows = Rows(groups(m.rows, home: home).flatMap { g in [groupHead(g.label, groupMeta(g))] + g.rows.map { server($0, now) } })
        let s = vstack([title, figs, vstack([head, block(p.ink, h: 1)], 5), rows])
        s.setCustomSpacing(8, after: title)
        s.setCustomSpacing(10, after: figs)
        return s
    }

    /// A table line in fixed columns: PID, Build (flexible), Up, Active, CPU, Mem, action.
    func line(_ pid: NSView, _ build: NSView, _ up: NSView, _ active: NSView, _ cpu: NSView, _ mem: NSView, _ action: NSView) -> NSView {
        build.setContentCompressionResistancePriority(.init(100), for: .horizontal)
        return hstack([fixed(pid, w: 40), stretch(build), fixed(up, w: 46), fixed(active, w: 56), fixed(cpu, w: 40), fixed(mem, w: 46), fixed(action, w: 62)], 6)
    }

    func server(_ r: ServerRow, _ now: Date) -> NSView {
        let id = r.id
        let row = ServerLine(serverCells(r, now: now), p, path: r.s.path, action: action(id)) { [unowned self] in m.quitClicked(id) }
        lines[id] = row
        return row
    }

    func action(_ id: ServerID) -> ServerLine.Action { m.stopping.contains(id) ? .stopping : m.confirming == id ? .confirm : .quit }

    func serverCells(_ r: ServerRow, now: Date) -> [String] {
        ["\(r.id.pid)", buildLabel(r.s.path, cli: m.cli), upText(r, now: now), activeText(r.lastActive, now: now), cpuText(r.cpu), memText(r.s.mem)]
    }

    /// A group's heading row: its label and, flush right, sessions and memory.
    func groupHead(_ label: String, _ meta: String) -> GroupLine {
        let g = GroupLine(label, meta, p)
        groupMetas[label] = g
        return g
    }

    func empty() -> NSView {
        let text = fixed(vstack([
            label("No servers running", 17, .b, p.ink, wrap: 230),
            label("Claude Code and Codex start one trello-mcp per session. Open a project that uses it and it shows here.", 13, p.muted, wrap: 230),
        ], 8), w: 230)
        return hstack([boardMark(72, p.track), text, spacer()], 16, top: true)    // the spacer takes the slack of the wider panel, so the mark keeps its shape
    }
}

/// The Quit button's fill, text and border: outline Quit, red Confirm, dim Quitting…; ink fill while hot.
func buttonColors(_ action: ServerLine.Action, hot: Bool, p: Pal) -> (bg: NSColor?, fg: NSColor, border: NSColor) {
    if hot && action != .stopping { return (p.ink, p.paper, p.ink) }
    return action == .confirm ? (swissRed, .white, swissRed) : (nil, p.ink, p.ink)
}

/// One server row drawn in a single view, its Quit button included: text fields, stacks and
/// constraints per cell cost about 7 ms and 1 MB per row (45 rows: 320 ms, 42 MB per render).
final class ServerLine: NSView {
    enum Action { case quit, confirm, stopping }
    var values: [String] { didSet { if values != oldValue { needsDisplay = true } } }    // pid, build, up, active, cpu, mem
    var action: Action {
        didSet {
            guard action != oldValue else { return }
            hot = false     // Confirm shows red; `inside` stays, so the pointer must leave and come back to turn it hot
            setNeedsDisplay(buttonRect)
            NSAccessibility.post(element: button, notification: .titleChanged)     // VoiceOver hears Quit turn into Confirm
        }
    }
    fileprivate let p: Pal, act: () -> Void
    private(set) var hot = false
    private var inside = false      // the pointer is over the button
    private var armed = false
    private lazy var button = Button(self)
    override var isFlipped: Bool { true }
    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: 31) }
    var buttonRect: NSRect { NSRect(x: bounds.width - 62, y: 4, width: 62, height: 22) }
    var title: String { action == .stopping ? "Quitting…" : action == .confirm ? "Confirm" : "Quit" }

    init(_ values: [String], _ p: Pal, path: String, action: Action, _ act: @escaping () -> Void) {
        self.values = values
        self.p = p
        self.action = action
        self.act = act
        super.init(frame: NSRect(x: 0, y: 0, width: inner, height: 31))
        toolTip = path
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .mouseMoved, .activeAlways, .inVisibleRect], owner: self))
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Column rects from the right: button 62, mem 46, cpu 40, active 56, up 46, with 6 pt gaps; pid 40 at the left, build takes the rest.
    override func draw(_ dirty: NSRect) {
        let w = bounds.width
        let cols: [(CGFloat, CGFloat, NSTextAlignment, W)] = [
            (0, 40, .right, .b), (46, w - 326, .left, .r), (w - 274, 46, .right, .r),
            (w - 222, 56, .right, .r), (w - 160, 40, .right, .r), (w - 114, 46, .right, .b),
        ]
        for (v, c) in zip(values, cols) { text(v, NSRect(x: c.0, y: 8, width: c.1, height: 16), 12, c.3, p.ink, c.2) }
        // The button: outline Quit, red Confirm, dim Quitting…; ink fill while hot.
        let b = buttonRect, enabled = action != .stopping
        let (bg, fg, bd) = buttonColors(action, hot: hot, p: p)
        let path = NSBezierPath(rect: b.insetBy(dx: 0.75, dy: 0.75))
        if let bg { bg.setFill(); path.fill() }
        bd.withAlphaComponent(enabled ? 1 : 0.4).setStroke()
        path.lineWidth = 1.5
        path.stroke()
        text(title, NSRect(x: b.minX, y: b.minY + 4, width: b.width, height: 14), 11, .b, fg.withAlphaComponent(enabled ? 1 : 0.4), .center)
        p.hair.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: w, height: 1).fill()
    }

    private func text(_ s: String, _ r: NSRect, _ size: CGFloat, _ w: W, _ c: NSColor, _ align: NSTextAlignment) {
        let ps = NSMutableParagraphStyle()
        ps.alignment = align
        ps.lineBreakMode = .byTruncatingTail
        NSAttributedString(string: s, attributes: [.font: hvFont(size, w), .foregroundColor: c, .paragraphStyle: ps]).draw(in: r)
    }

    func setHot(_ h: Bool) { if h != hot { hot = h; setNeedsDisplay(buttonRect) } }
    /// Acts only when the pointer enters or leaves the button, so a drift after the click keeps Confirm red.
    func pointerMoved(inside i: Bool) {
        if i != inside { inside = i; setHot(i) }
    }
    override func mouseMoved(with e: NSEvent) { pointerMoved(inside: buttonRect.contains(convert(e.locationInWindow, from: nil))) }
    override func mouseExited(with e: NSEvent) { inside = false; setHot(false) }
    // Like a button: fires on mouse-up inside, and a double-click is not Quit then Confirm.
    override func mouseDown(with e: NSEvent) {
        armed = e.clickCount == 1 && action != .stopping && buttonRect.contains(convert(e.locationInWindow, from: nil))
    }
    override func mouseUp(with e: NSEvent) {
        if armed, action != .stopping, buttonRect.contains(convert(e.locationInWindow, from: nil)) { act() }
        armed = false
    }

    // Accessibility: the row reads its cells; its button is a child that presses like the drawn one.
    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .group }
    override func accessibilityLabel() -> String? {
        "trello-mcp server \(values[0]), build \(values[1]), up \(values[2]), active \(values[3]), CPU \(values[4]), memory \(values[5])"
    }
    override func accessibilityChildren() -> [Any]? { [button] }
    override func accessibilityHitTest(_ point: NSPoint) -> Any? {
        NSAccessibility.screenRect(fromView: self, rect: buttonRect).contains(point) ? button : self
    }

    // Weak: an accessibility client may keep the element after a render frees its row.
    private final class Button: NSAccessibilityElement {
        weak var row: ServerLine?
        init(_ row: ServerLine) { self.row = row; super.init() }
        override func isAccessibilityElement() -> Bool { true }
        override func accessibilityRole() -> NSAccessibility.Role? { .button }
        override func accessibilityLabel() -> String? {
            row.map { "\($0.action == .confirm ? "Confirm quit of" : "Quit") trello-mcp server \($0.values[0])" }
        }
        override func isAccessibilityEnabled() -> Bool { row.map { $0.action != .stopping } ?? false }
        override func accessibilityParent() -> Any? { row }
        override func accessibilityFrame() -> NSRect { row.map { NSAccessibility.screenRect(fromView: $0, rect: $0.buttonRect) } ?? .zero }
        override func accessibilityPerformPress() -> Bool {
            guard let row, row.action != .stopping else { return false }
            row.act()
            return true
        }
    }
}

/// Server rows and group headings stacked by frame: in an NSStackView, 50 rows still cost ~70 ms of Auto Layout a render.
final class Rows: NSView {
    override var isFlipped: Bool { true }
    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: subviews.reduce(0) { $0 + $1.intrinsicContentSize.height }) }

    init(_ rows: [NSView]) {
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        rows.forEach(addSubview)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        var y: CGFloat = 0
        for v in subviews {
            let h = v.intrinsicContentSize.height
            v.frame = NSRect(x: 0, y: y, width: bounds.width, height: h)
            y += h
        }
    }
}

/// A group heading drawn in one view: the label (cut in the middle) and its meta flush right, hairline below.
final class GroupLine: NSView {
    var meta: String { didSet { if meta != oldValue { needsDisplay = true } } }
    private let name: String, p: Pal
    override var isFlipped: Bool { true }
    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: 35) }

    init(_ name: String, _ meta: String, _ p: Pal) {
        self.name = name
        self.meta = meta
        self.p = p
        super.init(frame: NSRect(x: 0, y: 0, width: inner, height: 35))
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirty: NSRect) {
        let ps = NSMutableParagraphStyle()
        ps.alignment = .right
        let m = NSAttributedString(string: meta, attributes: [.font: hvFont(12), .foregroundColor: p.muted, .paragraphStyle: ps])
        let mw = ceil(m.size().width)
        m.draw(in: NSRect(x: bounds.width - mw, y: 13, width: mw, height: 16))
        let np = NSMutableParagraphStyle()
        np.lineBreakMode = .byTruncatingMiddle      // a long folder keeps both ends
        NSAttributedString(string: name, attributes: [.font: hvFont(13, .b), .foregroundColor: p.ink, .paragraphStyle: np])
            .draw(in: NSRect(x: 0, y: 12, width: bounds.width - mw - 8, height: 17))
        p.hair.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
    }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .staticText }
    override func accessibilityLabel() -> String? { "\(name), \(meta)" }
}

private final class Root: NSView {
    var onAppearance: () -> Void = {}
    override var isFlipped: Bool { true }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        onAppearance()
    }
}
