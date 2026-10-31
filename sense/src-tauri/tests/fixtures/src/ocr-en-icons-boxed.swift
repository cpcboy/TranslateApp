// ocr-en-icons-boxed.png 的来源：下面 render(kind: "boxed", scale: 1, dark: false, pt: 11, seed: 1) 的输出。
// 用 AppKit 画随机的 SF Symbols 方框图标，种子固定，所以每次画出来一样：swiftc ocr-en-icons-boxed.swift && ./ocr-en-icons-boxed <输出目录>
// 同一份代码改 kind 还能画 stars / toolbar / inbox（星级、工具栏、带复选框的收件箱）。
import AppKit
let outdir = CommandLine.arguments[1]
let symbols = ["line.3.horizontal","square","checkmark.square","calendar","folder","doc","star","star.fill","gearshape","magnifyingglass","bell","person","house","trash","pencil","square.and.arrow.up","lock","eye","bookmark","arrow.down.circle","arrow.clockwise","plus","minus","xmark","checkmark","info.circle","clock","envelope","photo","play","pause","doc.on.doc","link","terminal","cloud","cart","tag","sidebar.left","camera","mic","square.grid.2x2","list.bullet","bold","italic","underline","heart","hand.thumbsup"]
struct SeededRNG: RandomNumberGenerator { var seed: UInt64; mutating func next() -> UInt64 { seed = seed &* 6364136223846793005 &+ 1442695040888963407; return seed } }
func render(name: String, scale: CGFloat, dark: Bool, pt: CGFloat, kind: String, seed: Int) {
    var rng = SeededRNG(seed: UInt64(seed) &* 7919)
    let w = 360, rows = 6, h = Int(CGFloat(rows) * (pt * 2.6)) + 20
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(CGFloat(w) * scale), pixelsHigh: Int(CGFloat(h) * scale), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    rep.size = NSSize(width: w, height: h)
    NSGraphicsContext.saveGraphicsState(); NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let bg = dark ? NSColor(calibratedWhite: 0.16, alpha: 1) : NSColor.white
    let fg = dark ? NSColor(calibratedWhite: 0.92, alpha: 1) : NSColor(calibratedWhite: 0.1, alpha: 1)
    let gold = NSColor(calibratedRed: 0.96, green: 0.65, blue: 0.14, alpha: 1)
    bg.setFill(); NSRect(x: 0, y: 0, width: w, height: h).fill()
    func sym(_ name: String, _ x: CGFloat, _ y: CGFloat, _ color: NSColor) -> CGFloat {
        let cfg = NSImage.SymbolConfiguration(pointSize: pt, weight: .regular)
        guard let img = NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(cfg) else { return 0 }
        let t = NSImage(size: img.size, flipped: false) { rect in img.draw(in: rect); color.set(); rect.fill(using: .sourceIn); return true }
        t.draw(in: NSRect(x: x, y: y, width: img.size.width, height: img.size.height)); return img.size.width
    }
    let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: pt), .foregroundColor: fg]
    for r in 0..<rows {
        let y = CGFloat(h) - 12 - CGFloat(r + 1) * (pt * 2.6)
        var x: CGFloat = 16
        switch kind {
        case "stars":
            let n = Int.random(in: 3...5, using: &rng)
            for i in 0..<5 { x += sym(i < n ? "star.fill" : "star", x, y, gold) + 1 }
            let texts = ["Not bad", "4.0 out of 5 stars", "(1,234 ratings)", "Great product", "Would buy again"]
            (texts[Int.random(in: 0..<texts.count, using: &rng)] as NSString).draw(at: NSPoint(x: x + 8, y: y), withAttributes: attrs)
        case "toolbar":
            let n = Int.random(in: 4...9, using: &rng)
            for _ in 0..<n { x += sym(symbols[Int.random(in: 0..<symbols.count, using: &rng)], x, y, fg) + pt * 0.9 }
        case "boxed":
            let n = Int.random(in: 3...7, using: &rng)
            for _ in 0..<n {
                let bw = pt * 2.0
                let path = NSBezierPath(roundedRect: NSRect(x: x, y: y - pt * 0.35, width: bw, height: bw), xRadius: 4, yRadius: 4)
                fg.setStroke(); path.lineWidth = 1; path.stroke()
                _ = sym(symbols[Int.random(in: 0..<symbols.count, using: &rng)], x + pt * 0.3, y + pt * 0.05, fg); x += bw + 4
            }
        case "inbox":
            x += sym("square", x, y, fg) + 6; x += sym("star", x, y, fg) + 10
            (["Google","GitHub","Alice Johnson","Stripe"][Int.random(in: 0..<4, using: &rng)] + "   Your weekly summary is ready" as NSString).draw(at: NSPoint(x: x, y: y), withAttributes: attrs)
        default: break
        }
    }
    NSGraphicsContext.restoreGraphicsState()
    try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "\(outdir)/\(name).png"))
}
_ = NSApplication.shared
var k = 0
for kind in ["stars","toolbar","boxed","inbox"] { for scale in [1.0, 2.0] { for dark in [false, true] { for pt in [11.0, 13.0, 15.0] { for seed in 1...4 {
    render(name: "sf2_\(kind)_\(Int(scale))x_\(dark ? "dark" : "light")_\(Int(pt))_\(seed)", scale: CGFloat(scale), dark: dark, pt: CGFloat(pt), kind: kind, seed: seed); k += 1
}}}}}
print("rendered", k)
