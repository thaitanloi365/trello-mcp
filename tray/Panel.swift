// The tray's panel in plain AppKit, after claude-usage's tray and its DESIGN.md (the Swiss
// instrument sheet): Helvetica Neue, ink on paper, rules not boxes, Swiss red only for alarms.
// AppKit, not SwiftUI, keeps that framework out of a menu bar process.
import AppKit

// MARK: - Model

/// The panel's state. A change of shape re-renders the panel while it shows; a tick (same servers,
/// new numbers) only updates text in place, which keeps hover and scroll.
final class PanelModel {
    var rows: [Row] = [] { didSet { sameShape(oldValue, rows) ? ticked() : changed() } }
    var open: ServerID? { didSet { changed() } }        // expanded row
    var error: String? { didSet { changed() } }
    var cap: CGFloat = 800 { didSet { changed() } }     // tallest the scrolling middle may be; Tray sets it from the screen
    var stats = "" { didSet { ticked() } }               // the tray's own CPU and memory
    var changed: () -> Void = {}                         // set by PanelController
    var ticked: () -> Void = {}                          // set by PanelController
    // Set by Tray.
    var onStop: (Row) -> Void = { _ in }
    var onLog: () -> Void = {}
    var onQuit: () -> Void = {}
}

// MARK: - Palette

struct Pal {
    let paper, ink, muted, hair, track, redInk, amber: NSColor
    func text(_ t: Tone) -> NSColor { t == .red ? redInk : t == .amber ? amber : ink }
    /// A server's CPU figure: the Threshold Rule.
    func fig(_ p: Double?) -> NSColor { text(tone(p)) }
    func status(_ s: Status) -> NSColor {
        switch s {
        case .stopping: redInk
        case .orphaned: amber
        case .busy: ink
        case .idle: muted
        }
    }
}

extension NSColor {
    convenience init(hex: UInt32) { self.init(srgbRed: CGFloat(hex >> 16 & 255) / 255, green: CGFloat(hex >> 8 & 255) / 255, blue: CGFloat(hex & 255) / 255, alpha: 1) }
}

let swissRed = NSColor(hex: 0xda291c)

/// DESIGN.md values; `inverted` is the totals block (claude-usage's `.cur`).
func pal(dark: Bool, inverted: Bool) -> Pal {
    switch (dark, inverted) {
    case (false, false): Pal(paper: NSColor(hex: 0xffffff), ink: NSColor(hex: 0x111111), muted: NSColor(hex: 0x5c5c5c), hair: NSColor(hex: 0xd4d4d4), track: NSColor(hex: 0xe6e6e6), redInk: NSColor(hex: 0xda291c), amber: NSColor(hex: 0xa85600))
    case (true, false): Pal(paper: NSColor(hex: 0x0d0d0d), ink: NSColor(hex: 0xf2f2f2), muted: NSColor(hex: 0xa3a3a3), hair: NSColor(hex: 0x333333), track: NSColor(hex: 0x2a2a2a), redInk: NSColor(hex: 0xff6a5c), amber: NSColor(hex: 0xf0a43a))
    case (false, true): Pal(paper: NSColor(hex: 0x111111), ink: NSColor(hex: 0xf2f2f2), muted: NSColor(hex: 0xa3a3a3), hair: NSColor(hex: 0x333333), track: NSColor(hex: 0x2e2e2e), redInk: NSColor(hex: 0xff6a5c), amber: NSColor(hex: 0xf0a43a))
    case (true, true): Pal(paper: NSColor(hex: 0xf2f2f2), ink: NSColor(hex: 0x111111), muted: NSColor(hex: 0x5c5c5c), hair: NSColor(hex: 0xd4d4d4), track: NSColor(hex: 0xd9d9d9), redInk: NSColor(hex: 0xda291c), amber: NSColor(hex: 0xa85600))
    }
}

// MARK: - Pieces

enum W { case r, m, b }

/// Helvetica Neue at a fixed size, with tabular digits.
func hvFont(_ size: CGFloat, _ w: W = .r) -> NSFont {
    let f = NSFont(name: w == .b ? "HelveticaNeue-Bold" : w == .m ? "HelveticaNeue-Medium" : "HelveticaNeue", size: size) ?? .systemFont(ofSize: size)
    let tnum: [NSFontDescriptor.FeatureKey: Int] = [.typeIdentifier: kNumberSpacingType, .selectorIdentifier: kMonospacedNumbersSelector]
    return NSFont(descriptor: f.fontDescriptor.addingAttributes([.featureSettings: [tnum]]), size: size) ?? f
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

/// Sets a layer property without Core Animation's implicit fade.
func still(_ f: () -> Void) {
    CATransaction.begin()
    CATransaction.setDisableActions(true)
    f()
    CATransaction.commit()
}

/// The board mark: three columns of different heights hanging from the top, like lists on a board.
let boardColumns: [CGFloat] = [1, 0.65, 0.8]

func boardMark(_ h: CGFloat, _ c: NSColor) -> NSView {
    hstack(boardColumns.map { block(c, w: h / 4, h: h * $0) }, h / 8, top: true)
}

/// A 3/4 ring turning every 0.7 s, unless Reduce Motion: the busy spinner.
func spinner(_ size: CGFloat, _ c: NSColor) -> NSView {
    let v = fixed(NSView(frame: NSRect(x: 0, y: 0, width: size, height: size)), w: size, h: size)
    v.wantsLayer = true
    let l = CAShapeLayer()
    l.frame = v.bounds
    l.path = CGPath(ellipseIn: v.bounds.insetBy(dx: 1, dy: 1), transform: nil)
    l.fillColor = nil
    l.strokeColor = c.cgColor
    l.lineWidth = 2
    l.strokeEnd = 0.75
    l.contentsScale = NSScreen.main?.backingScaleFactor ?? 2
    v.layer?.addSublayer(l)
    if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
        let a = CABasicAnimation(keyPath: "transform.rotation.z")
        a.byValue = -2 * Double.pi      // y-up layers: negative turns clockwise
        a.duration = 0.7
        a.repeatCount = .infinity
        l.add(a, forKey: "turn")
    }
    return v
}

/// A borderless button made of its own views; `paint` restyles it when it turns hot (hover or press) or back.
/// The focus ring is the system's.
final class Press: NSButton {
    var paint: ((Press, Bool) -> Void)? { didSet { repaint() } }
    override var isEnabled: Bool { didSet { repaint() } }
    private let content: NSView, pad: (CGFloat, CGFloat), minHeight: CGFloat, act: () -> Void
    private var hover = false, down = false
    private lazy var fit = content.fittingSize

    /// `pad` is (leading, trailing). `fill` stretches `content` across; otherwise it sits in the middle.
    init(_ content: NSView, pad: (CGFloat, CGFloat) = (0, 0), minHeight: CGFloat = 0, fill: Bool = false, _ act: @escaping () -> Void) {
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
        ] + (fill ? [
            content.leadingAnchor.constraint(equalTo: leadingAnchor, constant: pad.0),
            content.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -pad.1),
        ] : [
            content.centerXAnchor.constraint(equalTo: centerXAnchor, constant: (pad.0 - pad.1) / 2),
            content.leadingAnchor.constraint(greaterThanOrEqualTo: leadingAnchor, constant: pad.0),
        ]))
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

enum BtnKind { case outline, red }

/// A boxed button (the design's .btn): 2 pt border, ink fill while hot; 40% when disabled.
func btn(_ kind: BtnKind, _ p: Pal, _ title: String, size: CGFloat = 13, h: CGFloat = 36, pad: CGFloat = 14,
         enabled: Bool = true, _ act: @escaping () -> Void) -> Press {
    let l = label(title, size, .b, p.ink)
    let b = Press(l, pad: (pad, pad), minHeight: h, act)
    b.layer?.borderWidth = 2
    b.isEnabled = enabled
    b.alphaValue = enabled ? 1 : 0.4
    b.paint = { b, hot in
        let (bg, fg, bd): (NSColor, NSColor, NSColor) = switch (kind, hot) {
        case (.outline, false): (.clear, p.ink, p.ink)
        case (.red, false): (swissRed, .white, swissRed)
        default: (p.ink, p.paper, p.ink)
        }
        b.layer?.backgroundColor = bg.cgColor
        b.layer?.borderColor = bd.cgColor
        recolor(l, fg)
    }
    b.setAccessibilityLabel(title)
    return b
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

/// The panel's content: rebuilt from the model on each change of shape while it shows.
final class PanelController: NSViewController {
    let m: PanelModel
    private var queued = false
    // What a tick updates in place; filled by render().
    private struct RowRefs { let head: Press; let cpu, mem, status: NSTextField; let square: NSView? }
    private var rowRefs: [ServerID: RowRefs] = [:]
    private var figRefs: [NSTextField] = []     // servers, CPU, memory
    private var noteRef: NSTextField?
    private var statsRef: NSTextField?
    private var totalsRef: NSView?
    private var scrollRef: NSScrollView?
    private var p = pal(dark: false, inverted: false), inv = pal(dark: false, inverted: true)

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
        p = pal(dark: dark, inverted: false)
        inv = pal(dark: dark, inverted: true)
        rowRefs = [:]
        figRefs = []
        noteRef = nil
        let keptY = scrollRef?.contentView.bounds.origin.y ?? 0     // a re-render keeps the scroll position
        // Header and footer stay put; the middle scrolls once it outgrows the screen.
        let mid = vstack(middle(), 16)
        mid.widthAnchor.constraint(equalToConstant: 328).isActive = true
        let h = mid.fittingSize.height
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = h > m.cap
        scroll.verticalScrollElasticity = h > m.cap ? .automatic : .none
        scroll.documentView = mid
        scroll.contentView.scroll(to: NSPoint(x: 0, y: min(keptY, max(0, h - m.cap))))
        scroll.reflectScrolledClipView(scroll.contentView)
        scrollRef = scroll
        NSLayoutConstraint.activate([
            mid.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            mid.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            scroll.heightAnchor.constraint(equalToConstant: min(h, m.cap)),
        ])
        let col = vstack([header(), scroll, footer()], 16, inset: edges(16, 16, 16, 16))
        col.translatesAutoresizingMaskIntoConstraints = false
        view.subviews.forEach { $0.removeFromSuperview() }
        view.addSubview(col)
        NSLayoutConstraint.activate([
            col.widthAnchor.constraint(equalToConstant: 360),
            col.topAnchor.constraint(equalTo: view.topAnchor),
            col.leadingAnchor.constraint(equalTo: view.leadingAnchor),
        ])
        view.layer?.backgroundColor = p.paper.cgColor
        let size = col.fittingSize
        if let w = view.window {    // keep the top edge, under the status item
            w.setFrame(NSRect(x: w.frame.minX, y: w.frame.maxY - size.height, width: size.width, height: size.height), display: true)
        }
    }

    /// New numbers for the same rows: text, colors and VoiceOver labels in place.
    func tick() {
        guard view.window?.isVisible == true else { return }
        let now = Date()
        for r in m.rows {
            guard let x = rowRefs[r.id] else { continue }
            setText(x.cpu, cpuText(r.cpu), p.fig(r.cpu))
            setText(x.mem, memText(r.mem), p.ink)
            let c = p.status(r.status)
            setText(x.status, statusLine(r, now: now), c)
            if let sq = x.square { still { sq.layer?.backgroundColor = c.cgColor } }
            x.head.setAccessibilityLabel(rowSummary(r, now: now))
        }
        let t = totals(m.rows, now: now)
        if figRefs.count == 3 {
            setText(figRefs[1], cpuText(t.cpu), inv.ink)
            setText(figRefs[2], memText(t.mem), inv.ink)
        }
        totalsRef?.setAccessibilityLabel(totalsLabel(t))
        if let n = noteRef { setText(n, t.note, inv.muted) }
        if let s = statsRef, !m.stats.isEmpty { setText(s, m.stats, p.muted) }
    }

    func middle() -> [NSView] {
        var out: [NSView] = []
        if let e = m.error {
            let box = pad(label(e, 13, .b, p.redInk, wrap: 308), edges(10, 10, 10, 10))
            box.wantsLayer = true
            box.layer?.borderWidth = 2
            box.layer?.borderColor = p.redInk.cgColor
            out.append(box)
        }
        return out + (m.rows.isEmpty ? [empty()] : [totalsBlock(), table()])
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

    /// Servers, CPU and memory in total, inverted: ink ground, paper text.
    func totalsBlock() -> NSView {
        let t = totals(m.rows, now: Date())
        var figs: [Stack] = []
        for (name, value) in [("SERVERS", "\(m.rows.count)"), ("CPU", cpuText(t.cpu)), ("MEMORY", memText(t.mem))] {
            let f = label(value, 28, .b, em: -0.02, inv.ink)
            f.setContentCompressionResistancePriority(.init(100), for: .horizontal)
            figRefs.append(f)
            figs.append(vstack([label(name, 11, .m, em: 0.06, inv.muted), f], 4))
        }
        fixed(figs[0], w: 64)
        let figRow = hstack(figs, 16, top: true)     // the equal-width constraint needs a common ancestor
        figs[1].widthAnchor.constraint(equalTo: figs[2].widthAnchor).isActive = true
        let note = label(t.note, 12, inv.muted)
        note.setContentCompressionResistancePriority(.init(100), for: .horizontal)
        noteRef = note
        let s = vstack([figRow, vstack([block(inv.hair, h: 1), note], 7)], 12, inset: edges(16, 16, 16, 16))
        s.wantsLayer = true
        s.layer?.backgroundColor = inv.paper.cgColor
        s.setAccessibilityElement(true)
        s.setAccessibilityRole(.group)
        s.setAccessibilityLabel(totalsLabel(t))
        totalsRef = s
        return s
    }

    func totalsLabel(_ t: (cpu: Double?, mem: UInt64, note: String)) -> String {
        "\(m.rows.count) servers, CPU \(cpuText(t.cpu)), memory \(memText(t.mem)), \(t.note)"
    }

    func table() -> NSView {
        let col = { (s: String) in fixed(label(s, 11, .m, em: 0.06, self.p.muted, align: .right), w: 56) }
        let head = hstack([stretch(label("SERVERS", 11, .m, em: 0.06, p.muted)), col("CPU"), col("MEMORY")], 8)
        return vstack([vstack([head, block(p.ink, h: 1)], 5)] + m.rows.map(row))
    }

    /// Name and numbers, the status line under them; a click expands it.
    func row(_ r: Row) -> NSView {
        let open = m.open == r.id, id = r.id
        let chevron = fixed(NSImageView(image: NSImage(systemSymbolName: open ? "chevron.down" : "chevron.right", accessibilityDescription: nil)!
            .withSymbolConfiguration(.init(pointSize: 9, weight: .bold))!), w: 10)
        chevron.contentTintColor = p.ink
        let name = label(r.label, 13, .b, p.ink, cut: .byTruncatingMiddle)
        name.setContentCompressionResistancePriority(.init(100), for: .horizontal)
        let cpu = fixed(label(cpuText(r.cpu), 15, .b, p.fig(r.cpu), align: .right), w: 56)
        let mem = fixed(label(memText(r.mem), 15, .b, p.ink, align: .right), w: 56)
        let c = p.status(r.status)
        let word = label(statusLine(r, now: Date()), 11, .b, em: 0.04, c)
        word.setContentCompressionResistancePriority(.init(100), for: .horizontal)
        let square: NSView? = r.status == .stopping ? nil : block(c, w: 8, h: 8)
        let st = hstack([square ?? spinner(10, c), word, spacer()], 6)
        st.edgeInsets.left = 16
        let head = Press(vstack([hstack([chevron, name, spacer(), cpu, mem], 8), st], 3, inset: edges(7, 0, 7, 0)), fill: true) { [unowned self] in
            m.open = open ? nil : id
        }
        let ink = p.ink, red = p.redInk
        head.paint = { _, hot in
            recolor(name, hot ? red : ink)
            chevron.contentTintColor = hot ? red : ink
        }
        head.setAccessibilityLabel(rowSummary(r, now: Date()))
        head.setAccessibilityValue(open ? "expanded" : "collapsed")
        rowRefs[r.id] = RowRefs(head: head, cpu: cpu, mem: mem, status: word, square: square)
        return vstack([head] + (open ? [details(r)] : []) + [block(p.ink, h: 1)])
    }

    func details(_ r: Row) -> NSView {
        let lines: [NSView] = detailLines(r.proc, home: userHome, now: Date()).map { k, v in
            let value = label(v, 12, .r, p.ink, align: .right, cut: .byTruncatingMiddle)
            value.toolTip = v
            value.setContentCompressionResistancePriority(.init(100), for: .horizontal)
            return hstack([label(k, 12, p.muted), stretch(value)], 12)
        }
        let alarm = r.status == .orphaned || tone(r.cpu) == .red
        let stop = btn(alarm ? .red : .outline, p, "Stop server", size: 12, h: 32, pad: 10, enabled: r.status != .stopping) { [unowned self] in
            m.onStop(r)
        }
        stop.setAccessibilityLabel("Stop server \(r.label), PID \(r.id.pid)")
        let note = label("Its session loses Trello tools until it reconnects (/mcp in Claude Code).", 12, p.muted, wrap: 312)
        let col = vstack(lines + [hstack([stop, spacer()]), note], 4, inset: edges(0, 16, 12, 0))
        if let last = lines.last { col.setCustomSpacing(12, after: last) }
        return col
    }

    func empty() -> NSView {
        let text = fixed(vstack([
            label("No servers running", 17, .b, p.ink, wrap: 230),
            label("Claude Code and Codex start one trello-mcp per session. Open a project that uses it and it shows here.", 13, p.muted, wrap: 230),
        ], 8), w: 230)
        return hstack([boardMark(72, p.track), text], 16, top: true)
    }
}

private final class Root: NSView {
    var onAppearance: () -> Void = {}
    override var isFlipped: Bool { true }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        onAppearance()
    }
}
