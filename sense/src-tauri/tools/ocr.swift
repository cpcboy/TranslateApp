// SENSE 的离线 OCR 辅助程序：读一张 PNG，用系统 Vision 识别文字与位置。
//
// 为什么单独一个二进制：Vision 只有 Swift/ObjC 接口，Rust 侧用 objc2 绑定
// 易碎且难调；这里用 swiftc 在构建时编译成 sidecar，运行时不依赖工具链。
// 不联网、不上传图片、不花钱。
//
// 用法：sense-ocr <png 路径>
//       sense-ocr --selftest        （自检像素分析，不碰 Vision）
// 输出：JSON 数组，坐标是图片像素、原点左上：
//       [{"text":..., "x":..., "y":..., "w":..., "h":..., "conf":...,
//         "words":[{"t":..., "x":..., "w":...}],   // 逐词框（译文字号要按它反推）
//         "bg":[r,g,b], "fg":[r,g,b],              // 这一行底下的底色与文字颜色（sRGB）
//         "ink":{"x":...,"y":...,"w":...,"h":...}, // 原文笔画的紧包围盒（含越出 Vision 框的部分）
//         "room": <像素>,                          // 笔画右边到下一个非底色像素的空白宽度
//         "cover":{"x":...,"y":...,"w":...,"h":...}}] // 允许遮盖的范围：笔画外扩一点，碰到别的东西就停
// words / bg / fg / ink / room / cover 都可能缺省：分析不出来就不给，使用方要能退回旧做法。

import AppKit
import Foundation
import Vision

// MARK: - 像素分析

/// 整数像素矩形，原点左上。
struct PixelRect {
    var x: Int
    var y: Int
    var w: Int
    var h: Int
    var right: Int { x + w }
    var bottom: Int { y + h }
}

/// 一张图的 RGB 像素（sRGB，每像素 4 字节，第 4 字节不用），第 0 行在最上面。
///
/// 一律先转进 sRGB 再读：截图带的是显示器的色彩空间（新 Mac 多半是 Display P3），
/// 把 P3 的数字当 sRGB 用，饱和的底色会发灰。交给 CoreGraphics 做色彩匹配，读到的就是网页里同一个颜色该写的值。
struct Raster {
    let width: Int
    let height: Int
    let bytes: [UInt8]

    init?(cgImage: CGImage) {
        let width = cgImage.width
        let height = cgImage.height
        // 像素数封顶：一张图要占 4 字节 × 像素数的内存，分配失败会让整个 OCR 进程崩掉。
        // 4000 万像素（8K × 5K）以内的屏幕截图都够用，更大的不做像素分析，使用方退回「只有文字和位置」的旧做法
        guard width > 0, height > 0, width <= 20_000, height <= 20_000, width * height <= 40_000_000,
              let space = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
        var buffer = [UInt8](repeating: 0, count: width * height * 4)
        let drawn = buffer.withUnsafeMutableBytes { raw -> Bool in
            guard let context = CGContext(
                data: raw.baseAddress,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: width * 4,
                space: space,
                bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
            ) else { return false }
            context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard drawn else { return nil }
        self.width = width
        self.height = height
        self.bytes = buffer
    }

    @inline(__always)
    func rgb(_ x: Int, _ y: Int) -> (Int, Int, Int) {
        let index = (y * width + x) * 4
        return (Int(bytes[index]), Int(bytes[index + 1]), Int(bytes[index + 2]))
    }

    /// 把矩形夹进图片；夹完是空的就返回 nil。
    func clip(_ rect: PixelRect) -> PixelRect? {
        let left = max(0, rect.x)
        let top = max(0, rect.y)
        let right = min(width, rect.right)
        let bottom = min(height, rect.bottom)
        guard right > left, bottom > top else { return nil }
        return PixelRect(x: left, y: top, w: right - left, h: bottom - top)
    }
}

/// 一行文字底下的样式：给覆盖层「原位替换」用。
struct LineStyle {
    var bg: (Int, Int, Int)
    var fg: (Int, Int, Int)?
    var ink: PixelRect?
    var room: Int
    /// 允许遮盖的范围：笔画外扩一点（盖住抗锯齿的毛边），但每一侧只扩到下一个非底色像素为止
    var cover: PixelRect?
}

@inline(__always)
func colorDistance(_ a: (Int, Int, Int), _ b: (Int, Int, Int)) -> Int {
    abs(a.0 - b.0) + abs(a.1 - b.1) + abs(a.2 - b.2)
}

/// 一串颜色里，沿这个方向是不是「平滑地、均匀地变了一大截」——渐变，而不是「某处一步换了颜色」（色块的边）。
/// 首尾差得够多（≥ 30），且最大的一步不超过总变化的 6 成：渐变每一步只变一点，色块的边是一步变完。
func isSmoothGradient(_ colors: [(Int, Int, Int)]) -> Bool {
    guard colors.count >= 4, let first = colors.first, let last = colors.last else { return false }
    let total = colorDistance(first, last)
    guard total >= 30 else { return false }
    var biggest = 0
    for index in 1..<colors.count { biggest = max(biggest, colorDistance(colors[index - 1], colors[index])) }
    return biggest * 10 <= total * 6
}

/// 分析 Vision 框住的一行文字：底色、文字颜色、笔画的紧包围盒、笔画右边的空白、允许遮盖的范围。
///
/// 术语：`base` 是 Vision 的框；`window` 是框往四周外扩一圈留白（横向 max(4, h/2)、竖向 max(3, h/4)，含框内）。
/// - 底色取整个窗口里「出现最多的颜色」：文字笔画占不到一半，众数就是底色（框外的留白也算进来，
///   所以笔画占了框 8 成的粗体字也不会把众数拉成字色）。
/// - 笔画 = 与底色差得够多的像素；阈值跟着这一块的最大反差走，浅灰字和白字都认得出来，抗锯齿的淡边也算上
///   （淡边没盖住的话，译文四周会留一圈原文的毛边）。
/// - 线条不是笔画：「把 Vision 框的每一列（或每一行）都占满」的行 / 列——表格线、分隔线、隔壁色块的边——不算。
///   文字的行里总有字间缝，竖笔上下也总还留着框的空边，所以这个判据不会误伤 M、l 这样的高竖笔；
///   而线条哪怕从框的一侧一直延伸出去也逃不掉（早先按「横穿整个窗口」判断，起点落在窗口里的表格线就漏了）。
/// - Vision 的框偏紧，笔画有时会越出去（斜体、字尾）：框边外面紧挨着的一行 / 一列还有笔画就接着往外长，
///   长到空行 / 空列为止，最多长 max(2, 行高/8) 个像素——不能一路长进隔壁那一行。
/// - 渐变底：底色不是单一颜色。底色改取两端的平均色——远端离它最多是总变化的一半，只要这个一半不超过笔画阈值，
///   上面的分析照样成立；总变化 ≥ 2 倍阈值时远端会被当成笔画，分析不可信，这时保守处理：
///   整个框外扩着盖，不给笔画包围盒和字色（前端按底色的明暗选黑 / 白字）。
/// 分析不出来（框在图外）返回 nil；区域里没有笔画时只给底色。
func analyzeLine(_ raster: Raster, box: PixelRect) -> LineStyle? {
    guard let base = raster.clip(box) else { return nil }
    let marginX = max(4, base.h / 2)
    let marginY = max(3, base.h / 4)
    guard let window = raster.clip(PixelRect(
        x: base.x - marginX, y: base.y - marginY, w: base.w + 2 * marginX, h: base.h + 2 * marginY
    )) else { return nil }
    let pad = max(2, base.h / 8)

    // 0. 渐变底：看框外的留白带（文字不会进到那里）。沿长边均匀取 8 处，每处取中位数颜色，看底色是不是平滑地变了一大截
    func median(_ values: [Int]) -> Int { values.sorted()[values.count / 2] }
    func sample(_ x: Int, _ y: Int) -> (Int, Int, Int) { raster.rgb(x, y) }
    func horizontalProfile(rows: Range<Int>) -> [(Int, Int, Int)]? {
        guard rows.count >= 2, window.w >= 8 else { return nil }
        return (0..<8).map { step in
            let x = window.x + (window.w - 1) * step / 7
            let pixels = rows.map { sample(x, $0) }
            return (median(pixels.map { $0.0 }), median(pixels.map { $0.1 }), median(pixels.map { $0.2 }))
        }
    }
    func verticalProfile(columns: Range<Int>) -> [(Int, Int, Int)]? {
        guard columns.count >= 2, window.h >= 8 else { return nil }
        return (0..<8).map { step in
            let y = window.y + (window.h - 1) * step / 7
            let pixels = columns.map { sample($0, y) }
            return (median(pixels.map { $0.0 }), median(pixels.map { $0.1 }), median(pixels.map { $0.2 }))
        }
    }
    let profiles = [
        horizontalProfile(rows: window.y..<base.y),
        horizontalProfile(rows: base.bottom..<window.bottom),
        verticalProfile(columns: window.x..<base.x),
        verticalProfile(columns: base.right..<window.right),
    ]
    // 渐变的两端颜色（阈值要等下面算出来才能决定算不算数）
    let gradientEnds: (first: (Int, Int, Int), last: (Int, Int, Int))? = profiles.compactMap { $0 }
        .first(where: isSmoothGradient)
        .flatMap { profile in profile.first.flatMap { first in profile.last.map { (first, $0) } } }

    // 1. 底色：量化到每通道 4 位，数最多的那一格，再取这一格里真实像素的均值
    var counts = [Int](repeating: 0, count: 4096)
    var sumR = [Int](repeating: 0, count: 4096)
    var sumG = [Int](repeating: 0, count: 4096)
    var sumB = [Int](repeating: 0, count: 4096)
    for y in window.y..<window.bottom {
        for x in window.x..<window.right {
            let (r, g, b) = raster.rgb(x, y)
            let key = ((r >> 4) << 8) | ((g >> 4) << 4) | (b >> 4)
            counts[key] += 1
            sumR[key] += r
            sumG[key] += g
            sumB[key] += b
        }
    }
    var best = 0
    for key in 1..<4096 where counts[key] > counts[best] { best = key }
    let n = max(1, counts[best])
    var bg = (sumR[best] / n, sumG[best] / n, sumB[best] / n)
    // 这一格里某个精确颜色占了一半以上（纯色底）：用它本身，不用均值——
    // 均值会被格子里靠近边缘的淡色拉偏一两级（白底算成 254），遮盖块和原来的底差一点点就是一块看得见的补丁
    var exact: [UInt32: Int] = [:]
    for y in window.y..<window.bottom {
        for x in window.x..<window.right {
            let (r, g, b) = raster.rgb(x, y)
            guard ((r >> 4) << 8) | ((g >> 4) << 4) | (b >> 4) == best else { continue }
            exact[UInt32(r) << 16 | UInt32(g) << 8 | UInt32(b), default: 0] += 1
        }
    }
    // 并列时取数值小的，别让字典的遍历顺序决定结果
    if let top = exact.max(by: { $0.value != $1.value ? $0.value < $1.value : $0.key > $1.key }), top.value * 2 > n {
        bg = (Int(top.key >> 16), Int((top.key >> 8) & 0xff), Int(top.key & 0xff))
    }

    // 渐变底：用两端的平均色当底色，比「出现最多的那一格」更能代表整行。
    // 但出现最多的那一格要是占了窗口一半以上，底色其实是平的——框外的留白带只是碰上了容器的软边（圆角、阴影），
    // 或紧排文字里邻行的字形，形式上像渐变而已。这时误判的后果很糟（底色换成两端的平均，颜色整个反过来），不能当渐变。
    // 真渐变的颜色摊在好几格里（每格跨 16 级，一条 255 级的渐变每格只占几个百分点），到不了一半
    let flatShare = counts[best] * 2 > window.w * window.h
    var gradientMiddle: (Int, Int, Int)?
    if let ends = gradientEnds, !flatShare {
        let middle = ((ends.first.0 + ends.last.0) / 2, (ends.first.1 + ends.last.1) / 2, (ends.first.2 + ends.last.2) / 2)
        bg = middle
        gradientMiddle = middle
    }

    // 2. 笔画阈值
    var maxDistance = 0
    for y in window.y..<window.bottom {
        for x in window.x..<window.right {
            maxDistance = max(maxDistance, colorDistance(raster.rgb(x, y), bg))
        }
    }
    // 反差太小：几乎是纯色，没有可见的文字
    guard maxDistance >= 24 else { return LineStyle(bg: bg, fg: nil, ink: nil, room: 0, cover: nil) }
    let threshold = min(60, max(18, maxDistance * 12 / 100))
    func isInk(_ x: Int, _ y: Int) -> Bool { colorDistance(raster.rgb(x, y), bg) > threshold }

    // 渐变得太厉害（总变化 ≥ 2 倍阈值）：远端离平均色也超过阈值，会被当成笔画，下面的分析不可信，保守处理
    if let ends = gradientEnds, let middle = gradientMiddle, colorDistance(ends.first, ends.last) >= 2 * threshold {
        let cover = raster.clip(PixelRect(x: base.x - pad, y: base.y - pad, w: base.w + 2 * pad, h: base.h + 2 * pad))
        return LineStyle(bg: middle, fg: nil, ink: nil, room: 0, cover: cover)
    }

    // 贯穿整个 Vision 框的线：把框的每一列（或每一行）都占满的，不是文字，是表格线、分隔线、隔壁色块的边。
    // 文字的行里总有字间缝，竖笔上下也总还留着框的空边（Vision 的框两头各留 1–2 像素），M、l 这样的高竖笔也填不满。
    // （一开始按「占框高 9 成以上」判断，13px 的字框只有 12px 高，M 的竖笔占 92%，被误当成了分隔线）
    func isFullRow(_ y: Int) -> Bool {
        guard y >= 0, y < raster.height else { return false }
        for x in base.x..<base.right where !isInk(x, y) { return false }
        return true
    }
    func isFullColumn(_ x: Int) -> Bool {
        guard x >= 0, x < raster.width else { return false }
        for y in base.y..<base.bottom where !isInk(x, y) { return false }
        return true
    }
    // 但「占满框」还不够：大图上 Vision 的框常比字形矮，N、H、d、l 的竖笔和下划线也能占满一个框，
    // 它们最多越出框一点点；真正的线（表格线、色块的边）会一直延伸出去。所以线条 = 占满框，并且离框 max(4, 行高/2) 远的地方
    // 仍有笔画。包围盒和「往外长」只排除线条；字色统计仍排除所有占满框的（输入框里的竖线不该拉亮字色）
    let lineExtent = max(4, base.h / 2)
    func isLineRow(_ y: Int) -> Bool {
        guard isFullRow(y) else { return false }
        let before = base.x - lineExtent, after = base.right + lineExtent - 1
        return (before >= 0 && isInk(before, y)) || (after < raster.width && isInk(after, y))
    }
    func isLineColumn(_ x: Int) -> Bool {
        guard isFullColumn(x) else { return false }
        let above = base.y - lineExtent, below = base.bottom + lineExtent - 1
        return (above >= 0 && isInk(x, above)) || (below < raster.height && isInk(x, below))
    }

    // 区间下界大于上界时 Swift 的 `..<` 会直接崩溃，先判断
    func rowHasText(_ y: Int, _ x0: Int, _ x1: Int) -> Bool {
        guard y >= 0, y < raster.height, !isLineRow(y) else { return false }
        let low = max(0, x0), high = min(raster.width, x1)
        guard low < high else { return false }
        for x in low..<high where isInk(x, y) { return true }
        return false
    }
    func columnHasText(_ x: Int, _ y0: Int, _ y1: Int) -> Bool {
        guard x >= 0, x < raster.width, !isLineColumn(x) else { return false }
        let low = max(0, y0), high = min(raster.height, y1)
        guard low < high else { return false }
        for y in low..<high where isInk(x, y) { return true }
        return false
    }
    /// 这一列 / 行有没有任何与底色不同的像素（量右边的空白、外扩遮盖范围用：色块、分隔线都算「不空」）
    func columnHasAnyInk(_ x: Int, _ y0: Int, _ y1: Int) -> Bool {
        guard x >= 0, x < raster.width else { return false }
        let low = max(0, y0), high = min(raster.height, y1)
        guard low < high else { return false }
        for y in low..<high where isInk(x, y) { return true }
        return false
    }
    func rowHasAnyInk(_ y: Int, _ x0: Int, _ x1: Int) -> Bool {
        guard y >= 0, y < raster.height else { return false }
        let low = max(0, x0), high = min(raster.width, x1)
        guard low < high else { return false }
        for x in low..<high where isInk(x, y) { return true }
        return false
    }

    // 3. 笔画越出 Vision 框的部分：先竖着、再横着，往外长。
    //    长度封顶在行高的 1/8（最少 2 像素）：Vision 的框最多漏一两个像素，
    //    不封顶的话，紧挨着的边线、按钮边、隔壁一行都会被当成「这一行的笔画」长进来
    let reach = max(2, base.h / 8)
    var grown = base
    var steps = 0
    while steps < reach, rowHasText(grown.y - 1, grown.x, grown.right) { grown.y -= 1; grown.h += 1; steps += 1 }
    steps = 0
    while steps < reach, rowHasText(grown.bottom, grown.x, grown.right) { grown.h += 1; steps += 1 }
    steps = 0
    while steps < reach, columnHasText(grown.x - 1, grown.y, grown.bottom) { grown.x -= 1; grown.w += 1; steps += 1 }
    steps = 0
    while steps < reach, columnHasText(grown.right, grown.y, grown.bottom) { grown.w += 1; steps += 1 }
    guard let area = raster.clip(grown) else { return LineStyle(bg: bg, fg: nil, ink: nil, room: 0, cover: nil) }

    // 4. 紧包围盒与文字颜色：只数不是线条的行和列上的笔画像素（先把 area 里每行每列是不是线条算好，别在内层循环里重复算）
    let fullRows = (area.y..<area.bottom).map { isFullRow($0) }
    let fullColumns = (area.x..<area.right).map { isFullColumn($0) }
    let lineRows = (area.y..<area.bottom).map { isLineRow($0) }
    let lineColumns = (area.x..<area.right).map { isLineColumn($0) }
    var minX = Int.max, minY = Int.max, maxX = Int.min, maxY = Int.min
    var inkCount = 0
    var histogram = [Int](repeating: 0, count: 766)
    for y in area.y..<area.bottom where !lineRows[y - area.y] {
        for x in area.x..<area.right where !lineColumns[x - area.x] {
            let distance = colorDistance(raster.rgb(x, y), bg)
            guard distance > threshold else { continue }
            minX = min(minX, x); maxX = max(maxX, x)
            minY = min(minY, y); maxY = max(maxY, y)
            inkCount += 1
            // 字色统计用严格版：占满框的竖笔虽然算笔画，但输入框里那种占满框的竖线不能拉亮字色
            if !fullRows[y - area.y] && !fullColumns[x - area.x] { histogram[min(distance, 765)] += 1 }
        }
    }
    guard inkCount > 0 else { return LineStyle(bg: bg, fg: nil, ink: nil, room: 0, cover: nil) }
    let ink = PixelRect(x: minX, y: minY, w: maxX - minX + 1, h: maxY - minY + 1)

    // 文字颜色 = 反差最大的那 20% 笔画像素的均值（抗锯齿的过渡色不算数）
    let statCount = histogram.reduce(0, +)
    let wanted = max(3, statCount / 5)
    var cut = 765
    var collected = 0
    while cut > threshold {
        collected += histogram[cut]
        if collected >= wanted { break }
        cut -= 1
    }
    var fr = 0, fg = 0, fb = 0, fn = 0
    for y in ink.y..<ink.bottom where !fullRows[y - area.y] {
        for x in ink.x..<ink.right where !fullColumns[x - area.x] {
            let pixel = raster.rgb(x, y)
            guard colorDistance(pixel, bg) >= cut else { continue }
            fr += pixel.0; fg += pixel.1; fb += pixel.2; fn += 1
        }
    }
    let foreground: (Int, Int, Int)? = fn > 0 ? (fr / fn, fg / fn, fb / fn) : nil

    // 5. 右边的空白：从笔画右端往右数，一直是底色的列有多少。译文比原文长时，可以往这片空白里延伸
    var room = 0
    var column = ink.right
    while column < raster.width, !columnHasAnyInk(column, ink.y, ink.bottom) {
        room += 1
        column += 1
    }

    // 6. 允许遮盖的范围：笔画往外扩一点（盖住抗锯齿的毛边），外扩量按行高定（最少 2 像素），
    //    但每一侧只扩到下一个非底色像素为止——遮盖块只许盖原文，不许盖到隔壁的色块、分隔线、图标上
    var padLeft = 0, padRight = 0, padTop = 0, padBottom = 0
    while padLeft < pad, ink.x - padLeft - 1 >= 0, !columnHasAnyInk(ink.x - padLeft - 1, ink.y, ink.bottom) { padLeft += 1 }
    while padRight < pad, ink.right + padRight < raster.width, !columnHasAnyInk(ink.right + padRight, ink.y, ink.bottom) { padRight += 1 }
    while padTop < pad, ink.y - padTop - 1 >= 0, !rowHasAnyInk(ink.y - padTop - 1, ink.x, ink.right) { padTop += 1 }
    while padBottom < pad, ink.bottom + padBottom < raster.height, !rowHasAnyInk(ink.bottom + padBottom, ink.x, ink.right) { padBottom += 1 }
    let cover = PixelRect(x: ink.x - padLeft, y: ink.y - padTop, w: ink.w + padLeft + padRight, h: ink.h + padTop + padBottom)
    return LineStyle(bg: bg, fg: foreground, ink: ink, room: room, cover: cover)
}

// MARK: - 自检

/// `--selftest`：用合成的图片核对像素分析，不碰 Vision。失败时往 stderr 说清哪一条，退出码非 0。
func runSelfTest() -> Never {
    var failures: [String] = []
    func check(_ condition: Bool, _ message: String) { if !condition { failures.append(message) } }
    func near(_ a: (Int, Int, Int)?, _ b: (Int, Int, Int), _ tolerance: Int = 3) -> Bool {
        guard let a = a else { return false }
        return abs(a.0 - b.0) <= tolerance && abs(a.1 - b.1) <= tolerance && abs(a.2 - b.2) <= tolerance
    }

    /// 画一张 sRGB 的合成图：底色 + 若干实心矩形（CoreGraphics 的 y 轴朝上，这里换成朝下的坐标）
    func makeImage(width: Int, height: Int, bg: (Int, Int, Int), rects: [(PixelRect, (Int, Int, Int))]) -> Raster? {
        guard let space = CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(
                data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
              ) else { return nil }
        func fill(_ rect: CGRect, _ c: (Int, Int, Int)) {
            context.setFillColor(red: CGFloat(c.0) / 255, green: CGFloat(c.1) / 255, blue: CGFloat(c.2) / 255, alpha: 1)
            context.fill(rect)
        }
        fill(CGRect(x: 0, y: 0, width: width, height: height), bg)
        for (rect, color) in rects {
            fill(CGRect(x: rect.x, y: height - rect.bottom, width: rect.w, height: rect.h), color)
        }
        guard let image = context.makeImage() else { return nil }
        return Raster(cgImage: image)
    }

    // 1. 深色底、浅色字：底色、字色、紧包围盒、右边的空白；同时核对「第 0 行在最上面」
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 50, y: 20, w: 40, h: 8), (200, 200, 200)),
    ]) {
        check(raster.rgb(60, 22) == (200, 200, 200), "行序反了：(60,22) 应是笔画色，实际 \(raster.rgb(60, 22))")
        check(raster.rgb(60, 50) == (30, 30, 30), "(60,50) 应是底色，实际 \(raster.rgb(60, 50))")
        let style = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 48, h: 16))
        check(near(style?.bg, (30, 30, 30)), "深色底：底色 \(String(describing: style?.bg))")
        check(near(style?.fg, (200, 200, 200)), "深色底：字色 \(String(describing: style?.fg))")
        check(style?.ink?.x == 50 && style?.ink?.y == 20 && style?.ink?.w == 40 && style?.ink?.h == 8,
              "深色底：紧包围盒 \(String(describing: style?.ink))")
        check(style?.room == 110, "右边一直空到图边：room 应为 110，实际 \(String(describing: style?.room))")
        // 框高 16 → 外扩 2 像素，四周都是底色，扩满
        check(style?.cover?.x == 48 && style?.cover?.y == 18 && style?.cover?.w == 44 && style?.cover?.h == 12,
              "遮盖范围应是笔画外扩 2 像素：(48,18,44,12)，实际 \(String(describing: style?.cover))")
    } else { check(false, "合成图 1 画不出来") }

    // 2. 右边有别的东西：空白宽度停在它前面
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 50, y: 20, w: 40, h: 8), (200, 200, 200)),
        (PixelRect(x: 150, y: 18, w: 10, h: 12), (90, 140, 255)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 48, h: 16))
        check(style?.room == 60, "右边 150 处有别的东西：room 应为 60，实际 \(String(describing: style?.room))")
    } else { check(false, "合成图 2 画不出来") }

    // 3. 笔画越出 Vision 框：往外长到空列为止，但最多长「行高的 1/8」（这里框高 16 → 2 像素）
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 50, y: 20, w: 40, h: 8), (200, 200, 200)),
    ]) {
        let slight = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 36, h: 16)) // 框到 82，笔画到 90
        check(slight?.ink?.right == 84, "越出 8 像素：只长封顶的 2 像素，右边界应是 84，实际 \(String(describing: slight?.ink))")
        let flush = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 42, h: 16)) // 框到 88，笔画到 90：漏了 2 像素
        check(flush?.ink?.right == 90, "漏 2 像素：应长到笔画末端 90，实际 \(String(describing: flush?.ink))")
    } else { check(false, "合成图 3 画不出来") }

    // 3a. 文字被隔壁的色块截断（Vision 的框伸进了色块里）：色块的边是「整列都是」，不是笔画，
    //     包围盒只能到文字的末端；不然遮盖块会把隔壁的色块盖掉一截
    if let raster = makeImage(width: 200, height: 60, bg: (255, 255, 255), rects: [
        (PixelRect(x: 20, y: 22, w: 40, h: 8), (34, 34, 34)),         // 文字：x 20..<60
        (PixelRect(x: 64, y: 0, w: 136, h: 60), (45, 95, 209)),       // 右边的蓝色面板：从 x=64 到图边，整列都是
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 16, y: 18, w: 60, h: 16)) // 框右边伸到 76，进了面板 12 像素
        check(style?.ink?.x == 20 && style?.ink?.right == 60, "色块不是笔画：包围盒应是 x 20..<60，实际 \(String(describing: style?.ink))")
        check(style?.room == 4, "文字右边到色块只隔 4 像素：room 应为 4，实际 \(String(describing: style?.room))")
        check(near(style?.bg, (255, 255, 255)), "底色仍是白的，不是色块的蓝：\(String(describing: style?.bg))")
        // 文字右边只有 4 像素底色、外扩上限 2：扩 2；左右上下各自按自己的空白扩
        check(style?.cover?.right == 62, "遮盖范围右边只能扩到 62，实际 \(String(describing: style?.cover))")
    } else { check(false, "合成图 3a 画不出来") }

    // 3a'. 色块紧贴着文字（中间一个像素的缝都没有）：遮盖范围一点都不能往那一侧扩
    if let raster = makeImage(width: 200, height: 60, bg: (255, 255, 255), rects: [
        (PixelRect(x: 20, y: 22, w: 40, h: 8), (34, 34, 34)),
        (PixelRect(x: 60, y: 0, w: 140, h: 60), (45, 95, 209)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 16, y: 18, w: 60, h: 16))
        check(style?.ink?.right == 60 && style?.cover?.right == 60,
              "色块紧贴着：遮盖范围右边就是 60，实际 ink=\(String(describing: style?.ink)) cover=\(String(describing: style?.cover))")
        check(style?.cover?.x == 18, "左边是白底，照常外扩 2：18，实际 \(String(describing: style?.cover))")
    } else { check(false, "合成图 3a' 画不出来") }

    // 3a''. 色块紧贴在文字的左边（Vision 的框伸进了色块）：左边同样不能扩，包围盒也不能把色块的列算进来
    if let raster = makeImage(width: 200, height: 60, bg: (255, 255, 255), rects: [
        (PixelRect(x: 0, y: 0, w: 20, h: 60), (45, 95, 209)),
        (PixelRect(x: 20, y: 22, w: 40, h: 8), (34, 34, 34)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 16, y: 18, w: 60, h: 16))
        check(style?.ink?.x == 20 && style?.cover?.x == 20,
              "左边紧贴色块：包围盒和遮盖范围的左边都是 20，实际 ink=\(String(describing: style?.ink)) cover=\(String(describing: style?.cover))")
        check(style?.cover?.right == 62, "右边是白底：照常扩 2，实际 \(String(describing: style?.cover))")
    } else { check(false, "合成图 3a'' 画不出来") }

    // 3e. 紧框里的高竖笔（M、l、b 的竖）占了框高的 9 成多，但不是分隔线：框上下总还留着至少一行空边。
    //     这样的列要算进包围盒，不然笔画顶上那一行会漏在遮盖范围外面（真机上 13px 的 More 就漏过）
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 20, y: 11, w: 2, h: 11), (220, 220, 220)),   // 竖笔：11 行，框 12 行（y 10..<22），占 92%
        (PixelRect(x: 26, y: 15, w: 10, h: 7), (220, 220, 220)),   // 旁边矮一点的字
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 18, y: 10, w: 20, h: 12))
        check(style?.ink?.y == 11 && style?.ink?.x == 20,
              "占框高 92% 的竖笔不是分隔线：包围盒应含它，起点 (20,11)，实际 \(String(describing: style?.ink))")
    } else { check(false, "合成图 3e 画不出来") }

    // 3b. 紧挨着的别的东西（边线）不能被当成这一行的笔画长进来
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 50, y: 20, w: 40, h: 8), (200, 200, 200)),
        (PixelRect(x: 96, y: 0, w: 1, h: 60), (90, 90, 90)),   // 竖直的分隔线，离笔画 6 像素
        (PixelRect(x: 0, y: 36, w: 200, h: 1), (90, 90, 90)),  // 水平的分隔线，离笔画 8 像素
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 48, h: 16))
        check(style?.ink?.right == 90 && style?.ink?.bottom == 28,
              "分隔线不该长进来：笔画应是 x 50..<90、y 20..<28，实际 \(String(describing: style?.ink))")
        check(style?.room == 6, "右边 6 像素外是分隔线：room 应为 6，实际 \(String(describing: style?.room))")
    } else { check(false, "合成图 3b 画不出来") }

    // 3d. 紧贴着框底的整行横线（表格线、下划线）：往外长时要挡住，不能当成这一行文字的一部分
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 50, y: 20, w: 40, h: 8), (200, 200, 200)),
        (PixelRect(x: 0, y: 30, w: 200, h: 1), (110, 110, 110)),   // 框 y 16..<30，线紧挨着在 y=30
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 48, h: 14))
        check(style?.ink?.bottom == 28, "紧贴着的横线不该长进来：笔画应到 y=28，实际 \(String(describing: style?.ink))")
    } else { check(false, "合成图 3d 画不出来") }

    // 3c. 纯色底的底色要精确（白底不能算成 254）
    if let raster = makeImage(width: 200, height: 60, bg: (255, 255, 255), rects: [
        (PixelRect(x: 20, y: 20, w: 60, h: 10), (34, 34, 34)),
        (PixelRect(x: 20, y: 19, w: 60, h: 1), (250, 250, 250)),  // 笔画上沿一行几乎是白的过渡色，落在同一个量化格里
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 16, y: 16, w: 68, h: 18))
        let isWhite = style.map { $0.bg.0 == 255 && $0.bg.1 == 255 && $0.bg.2 == 255 } ?? false
        check(isWhite, "白底应是精确的 255，实际 \(String(describing: style?.bg))")
    } else { check(false, "合成图 3c 画不出来") }

    // 4. 浅色底、深色字
    if let raster = makeImage(width: 200, height: 60, bg: (255, 255, 255), rects: [
        (PixelRect(x: 20, y: 20, w: 60, h: 10), (34, 34, 34)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 16, y: 16, w: 68, h: 18))
        check(near(style?.bg, (255, 255, 255)), "浅色底：底色 \(String(describing: style?.bg))")
        check(near(style?.fg, (34, 34, 34)), "浅色底：字色 \(String(describing: style?.fg))")
    } else { check(false, "合成图 4 画不出来") }

    // 5. 彩色底（按钮）白字：底色要是按钮的蓝色，不是页面的白色
    if let raster = makeImage(width: 200, height: 60, bg: (255, 255, 255), rects: [
        (PixelRect(x: 10, y: 10, w: 180, h: 40), (45, 95, 209)),
        (PixelRect(x: 30, y: 24, w: 70, h: 10), (255, 255, 255)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 26, y: 20, w: 78, h: 18))
        check(near(style?.bg, (45, 95, 209)), "按钮蓝底：底色 \(String(describing: style?.bg))")
        check(near(style?.fg, (255, 255, 255)), "按钮蓝底：字色 \(String(describing: style?.fg))")
    } else { check(false, "合成图 5 画不出来") }

    // 6. 纯色区域：只给底色，不给笔画
    if let raster = makeImage(width: 100, height: 40, bg: (60, 60, 64), rects: []) {
        let style = analyzeLine(raster, box: PixelRect(x: 10, y: 10, w: 50, h: 14))
        check(near(style?.bg, (60, 60, 64)), "纯色：底色 \(String(describing: style?.bg))")
        check(style?.ink == nil && style?.fg == nil, "纯色：不该有笔画 \(String(describing: style?.ink))")
    } else { check(false, "合成图 6 画不出来") }

    // 6b. 抗锯齿：笔画上下各有一行过渡色（半透明的边）。字色要取笔画本身的颜色，不能被过渡色拉淡；
    //     紧包围盒则要把过渡色那一行也算进去，否则译文盖不住原文的毛边
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 50, y: 19, w: 40, h: 1), (115, 115, 115)),
        (PixelRect(x: 50, y: 20, w: 40, h: 6), (200, 200, 200)),
        (PixelRect(x: 50, y: 26, w: 40, h: 1), (115, 115, 115)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 48, h: 16))
        check(near(style?.fg, (200, 200, 200)), "抗锯齿：字色应是笔画本身的 200，实际 \(String(describing: style?.fg))")
        check(style?.ink?.y == 19 && style?.ink?.h == 8, "抗锯齿：包围盒要含过渡色那两行 \(String(describing: style?.ink))")
    } else { check(false, "合成图 6b 画不出来") }

    // ── 评审后补的用例 ──

    /// 横向或纵向的线性渐变底（逐列 / 逐行 1 像素宽的矩形），再叠上若干矩形
    func gradientRects(width: Int, height: Int, from: (Int, Int, Int), to: (Int, Int, Int), vertical: Bool = false)
        -> [(PixelRect, (Int, Int, Int))] {
        let steps = vertical ? height : width
        return (0..<steps).map { step in
            let t = Double(step) / Double(max(1, steps - 1))
            let color = (Int(Double(from.0) + Double(to.0 - from.0) * t),
                         Int(Double(from.1) + Double(to.1 - from.1) * t),
                         Int(Double(from.2) + Double(to.2 - from.2) * t))
            return (vertical ? PixelRect(x: 0, y: step, w: width, h: 1) : PixelRect(x: step, y: 0, w: 1, h: height), color)
        }
    }

    // 4a. 中等强度的渐变底（评审员复现的 #4f46e5 → #9333ea 横向渐变 + 白字）：底色取两端平均，
    //     笔画范围必须是整行文字——以前远端的像素离「出现最多的那一格」太远被当成笔画，包围盒只到 83%，原文尾巴露在译文旁
    if let raster = makeImage(width: 300, height: 60, bg: (79, 70, 229), rects:
        gradientRects(width: 300, height: 60, from: (79, 70, 229), to: (147, 51, 234)) + [
            (PixelRect(x: 40, y: 24, w: 220, h: 10), (255, 255, 255)),
        ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 36, y: 20, w: 228, h: 18))
        check(style?.ink?.x == 40 && style?.ink?.right == 260,
              "渐变底：笔画范围应是整行文字 x 40..<260，实际 \(String(describing: style?.ink))")
        check(near(style?.fg, (255, 255, 255)), "渐变底：字色应是白色，实际 \(String(describing: style?.fg))")
        check(near(style?.bg, (113, 60, 231), 12), "渐变底：底色应是两端的平均，实际 \(String(describing: style?.bg))")
    } else { check(false, "合成图 4a 画不出来") }

    // 4b. 很强的渐变：远端离平均色也超过笔画阈值，笔画判定不可信——保守处理：整个框外扩着盖、不给包围盒和字色
    if let raster = makeImage(width: 300, height: 60, bg: (30, 30, 60), rects:
        gradientRects(width: 300, height: 60, from: (30, 30, 60), to: (200, 60, 200)) + [
            (PixelRect(x: 40, y: 24, w: 220, h: 10), (255, 255, 255)),
        ]) {
        let box = PixelRect(x: 36, y: 20, w: 228, h: 18)
        let style = analyzeLine(raster, box: box)
        check(style?.ink == nil && style?.fg == nil, "很强的渐变：不给笔画包围盒和字色，实际 ink=\(String(describing: style?.ink)) fg=\(String(describing: style?.fg))")
        check((style?.cover?.x ?? 99) <= box.x && (style?.cover?.right ?? 0) >= box.right
              && (style?.cover?.y ?? 99) <= box.y && (style?.cover?.bottom ?? 0) >= box.bottom,
              "很强的渐变：遮盖范围必须把整个 Vision 框都盖住，实际 \(String(describing: style?.cover))")
    } else { check(false, "合成图 4b 画不出来") }

    // 4c. 纵向的强渐变同样要识别（「强」是相对窗口说的：窗口只有 26 行高，渐变要陡到窗口内的总变化 ≥ 2 倍阈值）
    if let raster = makeImage(width: 300, height: 80, bg: (0, 0, 0), rects:
        gradientRects(width: 300, height: 80, from: (0, 0, 0), to: (255, 120, 255), vertical: true) + [
            (PixelRect(x: 40, y: 34, w: 220, h: 10), (255, 255, 255)),
        ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 36, y: 30, w: 228, h: 18))
        check(style?.ink == nil && style?.cover != nil, "纵向强渐变：应走保守处理，实际 ink=\(String(describing: style?.ink))")
    } else { check(false, "合成图 4c 画不出来") }

    // 4d. 很轻的渐变（总变化 < 30）：当成纯色，照常分析
    if let raster = makeImage(width: 300, height: 60, bg: (100, 100, 100), rects:
        gradientRects(width: 300, height: 60, from: (100, 100, 100), to: (108, 108, 108)) + [
            (PixelRect(x: 40, y: 24, w: 220, h: 10), (240, 240, 240)),
        ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 36, y: 20, w: 228, h: 18))
        check(style?.ink?.x == 40 && near(style?.fg, (240, 240, 240)), "很轻的渐变：照常分析，实际 ink=\(String(describing: style?.ink)) fg=\(String(describing: style?.fg))")
    } else { check(false, "合成图 4d 画不出来") }

    // 5a. 表格线：横线从表格左边框一路延伸到窗口外，起点落在窗口里，还压进了 Vision 框的下沿——
    //     它贯穿整个框宽，不是笔画：包围盒只能是文字，右边的空白也不能被它截断
    if let raster = makeImage(width: 200, height: 80, bg: (255, 255, 255), rects: [
        (PixelRect(x: 10, y: 0, w: 1, h: 80), (190, 190, 190)),     // 表格左边框
        (PixelRect(x: 10, y: 36, w: 190, h: 1), (190, 190, 190)),   // 横线，在框内（框 y 20..<38）
        (PixelRect(x: 40, y: 24, w: 60, h: 10), (34, 34, 34)),      // 文字
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 36, y: 20, w: 68, h: 18))
        check(style?.ink?.x == 40 && style?.ink?.right == 100 && style?.ink?.y == 24 && style?.ink?.bottom == 34,
              "表格线不是笔画：包围盒应是 x 40..<100、y 24..<34，实际 \(String(describing: style?.ink))")
        check(style?.room == 100, "文字右边到图边一直是空白：room 应为 100，实际 \(String(describing: style?.room))")
    } else { check(false, "合成图 5a 画不出来") }

    // 5b. 粗体字把框填得很满（框里 67% 是字，上下各留着两行空边）：底色要靠框外的留白才取得对
    //     （只在框内取，众数会变成字色）
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 50, y: 16, w: 40, h: 16), (200, 200, 200)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 46, y: 14, w: 48, h: 20))
        check(near(style?.bg, (30, 30, 30)) && near(style?.fg, (200, 200, 200)),
              "粗体字把框填满：底色仍要取留白的颜色，实际 bg=\(String(describing: style?.bg)) fg=\(String(describing: style?.fg))")
    } else { check(false, "合成图 5b 画不出来") }

    // 6a. 阈值上限 60：白字黑底，最大反差 765，阈值按 12% 会到 91——淡边（反差 66）就被丢了，译文四周留一圈原文的毛边
    if let raster = makeImage(width: 200, height: 60, bg: (0, 0, 0), rects: [
        (PixelRect(x: 50, y: 19, w: 40, h: 1), (22, 22, 22)),       // 抗锯齿的淡边：反差 66
        (PixelRect(x: 50, y: 20, w: 40, h: 8), (255, 255, 255)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 48, h: 16))
        check(style?.ink?.y == 19, "阈值封顶 60：反差 66 的淡边要算笔画，包围盒起点应是 19，实际 \(String(describing: style?.ink))")
    } else { check(false, "合成图 6a 画不出来") }

    // 6b. 阈值下限 18：低反差的字（反差 99）阈值只有 18，噪点（反差 15）不能算笔画
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 50, y: 19, w: 40, h: 1), (35, 35, 35)),       // 噪点：反差 15
        (PixelRect(x: 50, y: 20, w: 40, h: 8), (63, 63, 63)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 48, h: 16))
        check(style?.ink?.y == 20, "阈值下限 18：反差 15 的噪点不算笔画，包围盒起点应是 20，实际 \(String(describing: style?.ink))")
    } else { check(false, "合成图 6b 画不出来") }

    // 6c. 最大反差不到 24：几乎是纯色，没有可见的文字，不能硬造出笔画
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 50, y: 20, w: 40, h: 8), (37, 37, 37)),       // 反差 21
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 46, y: 16, w: 48, h: 16))
        check(style?.ink == nil && style?.fg == nil, "反差不到 24：不该有笔画，实际 \(String(describing: style?.ink))")
    } else { check(false, "合成图 6c 画不出来") }

    // 6d. 文字中间夹着一条贯穿框高的白竖线（输入框里的分隔符）：字色要取文字的灰，不能被竖线拉亮。
    //     竖线要够宽（12 像素，占统计的 3 成），不然「严格 / 不严格」两种统计得出同一个阈值，用例就分不出来
    if let raster = makeImage(width: 200, height: 60, bg: (30, 30, 30), rects: [
        (PixelRect(x: 20, y: 22, w: 25, h: 8), (150, 150, 150)),
        (PixelRect(x: 45, y: 18, w: 12, h: 16), (255, 255, 255)),  // 12 像素宽、贯穿整个框高（框 y 18..<34）
        (PixelRect(x: 57, y: 22, w: 25, h: 8), (150, 150, 150)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 16, y: 18, w: 70, h: 16))
        check(near(style?.fg, (150, 150, 150), 4), "贯穿框高的竖线不是笔画：字色应是 150，实际 \(String(describing: style?.fg))")
    } else { check(false, "合成图 6d 画不出来") }

    // 7. 色彩管理：截图带的是显示器的色彩空间（Display P3），读出来的必须是转进 sRGB 之后的值——
    //    直接当 sRGB 用，饱和色会发灰。用 CoreGraphics 自己的转换结果当参照
    if let p3 = CGColorSpace(name: CGColorSpace.displayP3), let srgb = CGColorSpace(name: CGColorSpace.sRGB),
       let context = CGContext(data: nil, width: 4, height: 4, bitsPerComponent: 8, bytesPerRow: 0, space: p3,
                               bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue),
       let source = CGColor(colorSpace: p3, components: [0.8, 0.3, 0.3, 1]),
       let converted = source.converted(to: srgb, intent: .defaultIntent, options: nil)?.components, converted.count >= 3 {
        context.setFillColor(source)
        context.fill(CGRect(x: 0, y: 0, width: 4, height: 4))
        if let image = context.makeImage(), let raster = Raster(cgImage: image) {
            func byte(_ value: CGFloat) -> Int { Int((min(1, max(0, value)) * 255).rounded()) }
            let expected = (byte(converted[0]), byte(converted[1]), byte(converted[2]))
            check(near(raster.rgb(1, 1), expected, 2), "P3 → sRGB：应是 \(expected)，实际 \(raster.rgb(1, 1))")
            check(abs(expected.0 - 204) >= 6, "这条用例要能分得出「转了」和「没转」：P3 的 204 转成 sRGB 应差 6 以上，实际 \(expected.0)")
        } else { check(false, "P3 图读不出像素") }
    } else { check(false, "合成图 7（P3）画不出来") }

    // ── 第二轮评审后补的用例（我上一轮修复引入的两个回归）──

    // 8a. 平色底 + 软边（圆角 / 阴影）：留白带的颜色从外面的浅色经过一步半透明衰减才到按钮的蓝，
    //     两步各占总变化的约一半——满足「平滑渐变」的形式判据，但底色其实是平的（出现最多的那一格占了窗口大半）。
    //     误判的后果很糟：底色被换成两端的平均色，颜色整个反过来。必须仍按平色分析
    if let raster = makeImage(width: 200, height: 60, bg: (0, 122, 255), rects: [
        (PixelRect(x: 0, y: 0, w: 45, h: 60), (231, 231, 234)),      // 按钮外面的页面色
        (PixelRect(x: 45, y: 0, w: 11, h: 60), (115, 177, 244)),     // 一步半透明的阴影 / 圆角过渡
        (PixelRect(x: 60, y: 24, w: 50, h: 10), (255, 255, 255)),    // 白字
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 50, y: 20, w: 60, h: 18))
        check(near(style?.bg, (0, 122, 255), 4), "平色底 + 软边：底色应是按钮的蓝，实际 \(String(describing: style?.bg))")
        check(near(style?.fg, (255, 255, 255)), "平色底 + 软边：字色应是白，实际 \(String(describing: style?.fg))")
        check(style?.ink?.x == 60 && style?.ink?.right == 110, "平色底 + 软边：笔画范围应是 x 60..<110，实际 \(String(describing: style?.ink))")
    } else { check(false, "合成图 8a 画不出来") }

    // 8b. 紧框里的字：Vision 的框比字形还矮（大图上常见），N、H、d、l 的竖笔和下划线占满了框，
    //     但它们最多越出框一点点，不是线条——要算进笔画和遮盖范围，否则字尾的竖笔会露在译文旁边，room 也会被误报成 0
    if let raster = makeImage(width: 200, height: 60, bg: (255, 255, 255), rects: [
        (PixelRect(x: 40, y: 20, w: 3, h: 14), (34, 34, 34)),        // 竖笔：正好占满框高（框 y 20..<34）
        (PixelRect(x: 52, y: 26, w: 12, h: 3), (34, 34, 34)),        // 横画
        (PixelRect(x: 73, y: 20, w: 3, h: 14), (34, 34, 34)),        // 另一根竖笔：占满框高
        (PixelRect(x: 40, y: 33, w: 36, h: 1), (34, 34, 34)),        // 下划线：落在框的最后一行，只到文字两端
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 40, y: 20, w: 36, h: 14))
        check(style?.ink?.x == 40 && style?.ink?.right == 76 && style?.ink?.y == 20 && style?.ink?.bottom == 34,
              "占满框的竖笔 / 下划线要算笔画：包围盒应是 x 40..<76、y 20..<34，实际 \(String(describing: style?.ink))")
        check(style?.room == 124, "右边一直空到图边：room 应为 124，不能因为竖笔被剔掉而误报，实际 \(String(describing: style?.room))")
    } else { check(false, "合成图 8b 画不出来") }

    // 8c. 表格线从窗口里（框的左边几个像素处）才开始，一直延伸到图边、贯穿框宽：这是线条，不是笔画。
    //     这条用例钉住「按 Vision 框判断」——退回「横穿整个窗口」就会漏掉它（线的起点在窗口里，窗口左边一截是空的）
    if let raster = makeImage(width: 200, height: 80, bg: (255, 255, 255), rects: [
        (PixelRect(x: 30, y: 36, w: 170, h: 1), (190, 190, 190)),   // 起点 x=30 在窗口（27..）里、框（36..）外
        (PixelRect(x: 40, y: 24, w: 60, h: 10), (34, 34, 34)),
    ]) {
        let style = analyzeLine(raster, box: PixelRect(x: 36, y: 20, w: 68, h: 18))
        check(style?.ink?.x == 40 && style?.ink?.right == 100 && style?.ink?.bottom == 34,
              "起点在窗口里的表格线也是线条：包围盒应是 x 40..<100、y 24..<34，实际 \(String(describing: style?.ink))")
    } else { check(false, "合成图 8c 画不出来") }

    // 7. 框在图外 / 半个在图外 / 零尺寸：不能崩，该给 nil 的给 nil
    if let raster = makeImage(width: 100, height: 40, bg: (30, 30, 30), rects: [
        (PixelRect(x: 5, y: 5, w: 20, h: 8), (220, 220, 220)),
    ]) {
        check(analyzeLine(raster, box: PixelRect(x: 500, y: 500, w: 10, h: 10)) == nil, "框在图外应为 nil")
        check(analyzeLine(raster, box: PixelRect(x: -50, y: -50, w: 10, h: 10)) == nil, "框在图外（负坐标）应为 nil")
        check(analyzeLine(raster, box: PixelRect(x: 10, y: 10, w: 0, h: 0)) == nil, "零尺寸应为 nil")
        let edge = analyzeLine(raster, box: PixelRect(x: -10, y: -4, w: 40, h: 20))
        check(edge != nil && edge?.ink != nil, "框压着图边：只分析图内的部分")
        // 随机框：只要不崩
        var seed: UInt64 = 0x9E3779B97F4A7C15
        func next(_ bound: Int) -> Int {
            seed = seed &* 6364136223846793005 &+ 1442695040888963407
            return Int((seed >> 33) % UInt64(bound))
        }
        for _ in 0..<3000 {
            _ = analyzeLine(raster, box: PixelRect(x: next(200) - 50, y: next(120) - 40, w: next(90), h: next(50)))
        }
    } else { check(false, "合成图 7 画不出来") }

    if failures.isEmpty {
        print("selftest ok")
        exit(0)
    }
    for failure in failures { FileHandle.standardError.write("selftest 失败：\(failure)\n".data(using: .utf8)!) }
    exit(1)
}

// MARK: - 入口

let args = CommandLine.arguments
if args.count >= 2 && args[1] == "--selftest" { runSelfTest() }

guard args.count >= 2,
      let image = NSImage(contentsOfFile: args[1]),
      let tiff = image.tiffRepresentation,
      let rep = NSBitmapImageRep(data: tiff),
      let cg = rep.cgImage
else {
    FileHandle.standardError.write("读不到图片\n".data(using: .utf8)!)
    exit(2)
}

let width = CGFloat(cg.width)
let height = CGFloat(cg.height)

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true
request.recognitionLanguages = ["en-US", "zh-Hans"]
// 关掉自动语言检测，省一点时间；两种语言都明确给了。
request.automaticallyDetectsLanguage = false

let handler = VNImageRequestHandler(cgImage: cg, options: [:])
do {
    try handler.perform([request])
} catch {
    FileHandle.standardError.write("OCR 失败: \(error)\n".data(using: .utf8)!)
    exit(3)
}

// 像素分析读不出来（极端的图片格式）就不给 bg / fg / ink / room，使用方退回旧做法
let raster = Raster(cgImage: cg)

var out: [[String: Any]] = []
for observation in (request.results ?? []) {
    guard let candidate = observation.topCandidates(1).first else { continue }
    // 置信度太低的多数是图标、边线、噪点，留给页面反而更乱
    guard candidate.confidence >= 0.3 else { continue }
    let box = observation.boundingBox
    let x = Double(box.origin.x * width)
    let y = Double((1 - box.origin.y - box.size.height) * height)
    let w = Double(box.size.width * width)
    let h = Double(box.size.height * height)
    // 非有限数（NaN / ∞）写进 JSON 会让 JSONSerialization 抛 NSException——那是 Objective-C 异常，Swift 的 catch 接不住，
    // 整个辅助程序直接崩，截屏翻译就彻底不可用。Vision 实际不会给，但这一行零成本，宁可丢掉这一行
    guard [x, y, w, h, Double(candidate.confidence)].allSatisfy({ $0.isFinite }) else { continue }
    var line: [String: Any] = [
        "text": candidate.string,
        "x": x,
        "y": y,
        "w": w,
        "h": h,
        "conf": Double(candidate.confidence),
    ]

    // 逐词框：按空白切词，各取 Vision 给的子串框。译文字号靠它反推，
    // 行首的图标（被读成 ©、•、C 之类的单个字符）自己成一个词，不会混进文字的宽度里
    var words: [[String: Any]] = []
    let text = candidate.string
    var cursor = text.startIndex
    while cursor < text.endIndex {
        var end = cursor
        while end < text.endIndex && !text[end].isWhitespace { end = text.index(after: end) }
        if end > cursor, let wordBox = try? candidate.boundingBox(for: cursor..<end) {
            let rect = wordBox.boundingBox
            let wordX = Double(rect.origin.x * width)
            let wordW = Double(rect.size.width * width)
            guard wordX.isFinite, wordW.isFinite else { cursor = end < text.endIndex ? text.index(after: end) : end; continue }
            words.append([
                "t": String(text[cursor..<end]),
                "x": wordX,
                "w": wordW,
            ])
        }
        cursor = end < text.endIndex ? text.index(after: end) : end
    }
    if !words.isEmpty { line["words"] = words }

    // Int(Double) 遇到超出范围的值会直接崩溃：先夹进一个宽松的范围（超出图片的部分 analyzeLine 自己会裁掉）
    func pixel(_ value: Double) -> Int { Int(max(-1e7, min(1e7, value))) }
    if let raster = raster,
       let style = analyzeLine(raster, box: PixelRect(
           x: pixel(x.rounded(.down)), y: pixel(y.rounded(.down)),
           w: pixel(w.rounded(.up)), h: pixel(h.rounded(.up))
       )) {
        line["bg"] = [style.bg.0, style.bg.1, style.bg.2]
        if let fg = style.fg { line["fg"] = [fg.0, fg.1, fg.2] }
        if let ink = style.ink {
            line["ink"] = ["x": ink.x, "y": ink.y, "w": ink.w, "h": ink.h]
            line["room"] = style.room
        }
        if let cover = style.cover { line["cover"] = ["x": cover.x, "y": cover.y, "w": cover.w, "h": cover.h] }
    }
    out.append(line)
}

let data = try JSONSerialization.data(withJSONObject: out, options: [])
FileHandle.standardOutput.write(data)
