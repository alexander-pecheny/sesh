import ImageIO
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// One image or video written into the app's temp directory, ready for the core to send.
struct PreparedUpload {
    let local: URL
    let name: String
}

/// Turns what the picker handed back into files the core can read. Images are downscaled
/// and re-encoded unless the user turned that off; videos are never touched, because
/// reading them is the Host's business.
enum Uploads {
    static let longestEdge = 1568
    static let quality = 0.8

    static func text(for paths: [String]) -> String {
        paths.map(quoted).joined(separator: " ") + " "
    }

    /// Single quotes only when the path holds something a shell would read. Our names
    /// never do; an odd home directory might.
    static func quoted(_ path: String) -> String {
        let safe = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._/-")
        guard path.unicodeScalars.contains(where: { !safe.contains($0) }) else { return path }
        return "'" + path.replacingOccurrences(of: "'", with: #"'\''"#) + "'"
    }

    static func prepare(_ results: [PHPickerResult], compress: Bool) async throws -> [PreparedUpload] {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("uploads")
        try? FileManager.default.removeItem(at: directory)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        let stamp = Self.stamp.string(from: Date())
        var prepared: [PreparedUpload] = []
        for (index, result) in results.enumerated() {
            let provider = result.itemProvider
            let base = provider.suggestedName ?? "image-\(index + 1)"
            let video = provider.hasItemConformingToTypeIdentifier(UTType.movie.identifier)
            let source = try await file(provider, type: video ? .movie : imageType(provider))
            let target: URL
            if !video, compress {
                target = directory.appendingPathComponent("\(stamp)-\(base).jpg")
                try shrink(source, to: target)
            } else {
                let suffix = source.pathExtension.isEmpty ? "dat" : source.pathExtension
                target = directory.appendingPathComponent("\(stamp)-\(base).\(suffix)")
                try FileManager.default.moveItem(at: source, to: target)
            }
            prepared.append(PreparedUpload(local: target, name: target.lastPathComponent))
        }
        return prepared
    }

    static func discard(_ files: [PreparedUpload]) {
        files.forEach { try? FileManager.default.removeItem(at: $0.local) }
    }

    /// HEIC becomes JPEG even when the user turned compression off: that is about the Host
    /// being able to open the file, not about size. iOS does the transcode itself.
    private static func imageType(_ provider: NSItemProvider) -> UTType {
        let types = provider.registeredTypeIdentifiers.compactMap(UTType.init)
        guard let native = types.first(where: { $0.conforms(to: .image) }) else { return .jpeg }
        return native.conforms(to: .heic) || native.conforms(to: .heif) ? .jpeg : native
    }

    /// The URL the provider gives is gone once the handler returns, so it is moved out.
    private static func file(_ provider: NSItemProvider, type: UTType) async throws -> URL {
        try await withCheckedThrowingContinuation { continuation in
            provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, error in
                guard let url else {
                    continuation.resume(throwing: error ?? Failure.unreadable)
                    return
                }
                let kept = FileManager.default.temporaryDirectory
                    .appendingPathComponent(UUID().uuidString)
                    .appendingPathExtension(url.pathExtension)
                continuation.resume(with: Result { try FileManager.default.copyItem(at: url, to: kept) }
                    .map { kept })
            }
        }
    }

    /// ImageIO resizes and re-encodes without ever holding the full-size bitmap, and
    /// applies the EXIF rotation on the way so the Host sees the photo the right way up.
    private static func shrink(_ source: URL, to target: URL) throws {
        defer { try? FileManager.default.removeItem(at: source) }
        guard let reader = CGImageSourceCreateWithURL(source as CFURL, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(reader, 0, [
                  kCGImageSourceCreateThumbnailFromImageAlways: true,
                  kCGImageSourceCreateThumbnailWithTransform: true,
                  kCGImageSourceThumbnailMaxPixelSize: longestEdge,
              ] as CFDictionary),
              let writer = CGImageDestinationCreateWithURL(
                  target as CFURL, UTType.jpeg.identifier as CFString, 1, nil)
        else { throw Failure.unreadable }
        CGImageDestinationAddImage(writer, image, [
            kCGImageDestinationLossyCompressionQuality: quality,
        ] as CFDictionary)
        guard CGImageDestinationFinalize(writer) else { throw Failure.unreadable }
    }

    private static let stamp: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyyMMdd-HHmmss"
        return formatter
    }()

    enum Failure: LocalizedError {
        case unreadable
        var errorDescription: String? { "the photo library would not give up that file" }
    }
}

/// How far the Upload in flight has got, across the whole batch. `ProgressView`'s circular
/// style on iOS ignores its value and spins, so the ring is drawn.
struct UploadRing: View {
    let fraction: Double
    let colour: Color

    var body: some View {
        Circle()
            .trim(from: 0, to: max(fraction, 0.02))
            .stroke(colour, style: .init(lineWidth: 2.5, lineCap: .round))
            .rotationEffect(.degrees(-90))
            .frame(width: 16, height: 16)
    }
}

/// The system picker, out of process, so there is no permission to ask for.
struct PhotoPicker: UIViewControllerRepresentable {
    let picked: ([PHPickerResult]) -> Void

    func makeUIViewController(context: Context) -> PHPickerViewController {
        var configuration = PHPickerConfiguration()
        configuration.filter = .any(of: [.images, .videos])
        configuration.selectionLimit = 0
        configuration.preferredAssetRepresentationMode = .current
        let controller = PHPickerViewController(configuration: configuration)
        controller.delegate = context.coordinator
        return controller
    }

    func updateUIViewController(_ controller: PHPickerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(picked: picked) }

    final class Coordinator: NSObject, PHPickerViewControllerDelegate {
        private let picked: ([PHPickerResult]) -> Void

        init(picked: @escaping ([PHPickerResult]) -> Void) { self.picked = picked }

        func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
            picked(results)
        }
    }
}
