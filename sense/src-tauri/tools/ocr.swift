// SENSE 的离线 OCR 辅助程序：读一张 PNG，用系统 Vision 识别文字与位置。
//
// 为什么单独一个二进制：Vision 只有 Swift/ObjC 接口，Rust 侧用 objc2 绑定
// 易碎且难调；这里用 swiftc 在构建时编译成 sidecar，运行时不依赖工具链。
// 不联网、不上传图片、不花钱。
//
// 用法：sense-ocr <png 路径>
// 输出：JSON 数组 [{"text":..., "x":..., "y":..., "w":..., "h":..., "conf":...}]
//       坐标是图片像素、原点左上。

import AppKit
import Foundation
import Vision

let args = CommandLine.arguments
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

var out: [[String: Any]] = []
for observation in (request.results ?? []) {
    guard let candidate = observation.topCandidates(1).first else { continue }
    // 置信度太低的多数是图标、边线、噪点，留给页面反而更乱
    guard candidate.confidence >= 0.3 else { continue }
    let box = observation.boundingBox
    out.append([
        "text": candidate.string,
        "x": Double(box.origin.x * width),
        "y": Double((1 - box.origin.y - box.size.height) * height),
        "w": Double(box.size.width * width),
        "h": Double(box.size.height * height),
        "conf": Double(candidate.confidence),
    ])
}

let data = try JSONSerialization.data(withJSONObject: out, options: [])
FileHandle.standardOutput.write(data)
