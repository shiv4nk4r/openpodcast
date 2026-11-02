import Foundation
import Speech
import AVFoundation

/// CLI: macos_transcribe <wav-or-audio-path>
/// Prints JSON: { "text": "...", "segments": [ { "text", "start", "end" } ] }

struct Seg: Codable {
    let text: String
    let start: Double
    let end: Double
}

struct Out: Codable {
    let text: String
    let segments: [Seg]
}

enum Fail: Int32 {
    case usage = 2
    case auth = 3
    case unavailable = 4
    case empty = 5
    case error = 6
}

func die(_ code: Fail, _ message: String) -> Never {
    FileHandle.standardError.write(Data("\(message)\n".utf8))
    fflush(stderr)
    exit(code.rawValue)
}

guard CommandLine.arguments.count >= 2 else {
    die(.usage, "Usage: macos_transcribe <audio-file>")
}

let path = CommandLine.arguments[1]
let url = URL(fileURLWithPath: path)
guard FileManager.default.fileExists(atPath: path) else {
    die(.usage, "File not found: \(path)")
}

// Authorization — must run with a runloop so the system dialog can appear.
let authSem = DispatchSemaphore(value: 0)
var authStatus = SFSpeechRecognizer.authorizationStatus()
if authStatus == .notDetermined {
    DispatchQueue.main.async {
        SFSpeechRecognizer.requestAuthorization { status in
            authStatus = status
            authSem.signal()
        }
    }
    let authDeadline = Date().addingTimeInterval(90)
    while authSem.wait(timeout: .now() + .milliseconds(50)) != .success {
        RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05))
        if Date() > authDeadline {
            die(.auth, "Timed out waiting for Speech Recognition permission.")
        }
    }
}

switch authStatus {
case .authorized:
    break
case .denied:
    die(.auth, "Speech recognition permission denied. System Settings → Privacy & Security → Speech Recognition → enable OpenPodcast (or macos_transcribe).")
case .restricted:
    die(.auth, "Speech recognition is restricted on this Mac.")
default:
    die(.auth, "Speech recognition not authorized (status=\(authStatus.rawValue)). Enable it in System Settings → Privacy & Security → Speech Recognition.")
}

let localeIds = [
    Locale.current.identifier,
    "en-US",
    "en-GB",
    "en_US",
    "en",
]
var recognizer: SFSpeechRecognizer?
for id in localeIds {
    if let r = SFSpeechRecognizer(locale: Locale(identifier: id)), r.isAvailable {
        recognizer = r
        break
    }
}
guard let recognizer else {
    die(.unavailable, "No Apple Speech recognizer available. Check language settings / network.")
}

// Prefer a mono 16k WAV that Rust already prepared; also accept original.
let request = SFSpeechURLRecognitionRequest(url: url)
request.shouldReportPartialResults = false
request.taskHint = .dictation
// Do NOT force on-device — missing models cause empty failures.
if #available(macOS 13, *) {
    request.requiresOnDeviceRecognition = false
}

let sem = DispatchSemaphore(value: 0)
var finalResult: SFSpeechRecognitionResult?
var taskError: Error?
var sawPartial = false

recognizer.defaultTaskHint = .dictation
let task = recognizer.recognitionTask(with: request) { result, error in
    if let error {
        taskError = error
        sem.signal()
        return
    }
    if let result {
        sawPartial = true
        if result.isFinal {
            finalResult = result
            sem.signal()
        }
    }
}

let started = Date()
while sem.wait(timeout: .now() + .milliseconds(50)) != .success {
    RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05))
    if Date().timeIntervalSince(started) > 180 {
        task.cancel()
        die(.error, "Speech recognition timed out after 180s")
    }
}

if let taskError {
    let ns = taskError as NSError
    die(.error, "Speech recognition failed: \(taskError.localizedDescription) (domain=\(ns.domain) code=\(ns.code))")
}

guard let finalResult else {
    if sawPartial {
        die(.empty, "Speech recognition ended without a final result")
    }
    die(.empty, "No speech recognized in this audio")
}

let transcription = finalResult.bestTranscription
var segments: [Seg] = transcription.segments.compactMap { s in
    let text = s.substring.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !text.isEmpty else { return nil }
    return Seg(
        text: text,
        start: s.timestamp,
        end: s.timestamp + max(s.duration, 0.15)
    )
}

let full = transcription.formattedString.trimmingCharacters(in: .whitespacesAndNewlines)
if segments.isEmpty && !full.isEmpty {
    let asset = AVURLAsset(url: url)
    let dur = CMTimeGetSeconds(asset.duration)
    let end = dur.isFinite && dur > 0 ? dur : 3.0
    segments = [Seg(text: full, start: 0, end: end)]
}

if full.isEmpty && segments.isEmpty {
    die(.empty, "No speech recognized in this audio")
}

let payload = Out(
    text: full.isEmpty ? segments.map(\.text).joined(separator: " ") : full,
    segments: segments
)
do {
    let data = try JSONEncoder().encode(payload)
    if let s = String(data: data, encoding: .utf8) {
        print(s)
        exit(0)
    }
    die(.error, "Failed to encode transcription JSON")
} catch {
    die(.error, "Failed to encode transcription JSON: \(error.localizedDescription)")
}
