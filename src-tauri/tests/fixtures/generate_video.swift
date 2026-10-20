// Independent synthetic H.264 fixtures using Apple's encoder, not bundled FFmpeg.
// Run on macOS: swift -module-cache-path /private/tmp/recast-swift-cache generate_video.swift <fixture-directory>
import AVFoundation
import Foundation

let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
for (name, times, rotated) in [
    ("apple-h264.mp4", (0..<60).map { Int64($0 * 1001) }, false),
    ("apple-vfr-rotated.mov", [Int64(0), 1000, 3100, 4000, 10000, 17000, 20000, 28000, 30000], true),
] {
    let output = directory.appendingPathComponent(name)
    try? FileManager.default.removeItem(at: output)
    let writer = try AVAssetWriter(outputURL: output, fileType: rotated ? .mov : .mp4)
    let input = AVAssetWriterInput(mediaType: .video, outputSettings: [
        AVVideoCodecKey: AVVideoCodecType.h264,
        AVVideoWidthKey: 64, AVVideoHeightKey: 48,
        AVVideoCompressionPropertiesKey: [
            AVVideoAverageBitRateKey: 200000,
            AVVideoAllowFrameReorderingKey: false,
            AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
        ],
    ])
    if rotated { input.transform = CGAffineTransform(rotationAngle: .pi / 2) }
    let adapter = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: input,
        sourcePixelBufferAttributes: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
                                     kCVPixelBufferWidthKey as String: 64, kCVPixelBufferHeightKey as String: 48])
    writer.add(input)
    guard writer.startWriting() else { fatalError("\(writer.error!)") }
    writer.startSession(atSourceTime: .zero)
    for (frame, time) in times.enumerated() {
        while !input.isReadyForMoreMediaData { Thread.sleep(forTimeInterval: 0.001) }
        var buffer: CVPixelBuffer?
        precondition(CVPixelBufferPoolCreatePixelBuffer(nil, adapter.pixelBufferPool!, &buffer) == kCVReturnSuccess)
        let pixels = buffer!
        CVPixelBufferLockBaseAddress(pixels, [])
        let base = CVPixelBufferGetBaseAddress(pixels)!.assumingMemoryBound(to: UInt8.self)
        let stride = CVPixelBufferGetBytesPerRow(pixels)
        for y in 0..<48 { for x in 0..<64 {
            let p = base.advanced(by: y * stride + x * 4)
            p[0] = UInt8((x * 3 + frame * 11) % 256)
            p[1] = UInt8((y * 5 + frame * 17) % 256)
            p[2] = x < 20 && y < 20 ? 240 : 20
            p[3] = 255
        }}
        CVPixelBufferUnlockBaseAddress(pixels, [])
        precondition(adapter.append(pixels, withPresentationTime: CMTime(value: time, timescale: 30000)), "\(String(describing: writer.error))")
    }
    input.markAsFinished()
    writer.endSession(atSourceTime: CMTime(value: times.last! + 1001, timescale: 30000))
    let done = DispatchSemaphore(value: 0)
    writer.finishWriting { done.signal() }
    done.wait()
    precondition(writer.status == .completed, "\(String(describing: writer.error))")
    print("Created \(name)")
}
