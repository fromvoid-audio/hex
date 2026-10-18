// Internal launcher adapter. Never opens an application or targets one by name.
import AppKit
import Foundation

struct Identity: Codable, Equatable {
    let pid: Int32
    let started: String
    let bundle: String
    let executable: String
    let identifier: String
}

func identity(_ app: NSRunningApplication) throws -> Identity {
    guard !app.isTerminated, let date = app.launchDate,
          let bundle = app.bundleURL, let executable = app.executableURL,
          let identifier = app.bundleIdentifier else {
        throw Failure.message("application identity is unavailable")
    }
    return Identity(pid: app.processIdentifier,
                    started: String(format: "%.9f", date.timeIntervalSince1970),
                    bundle: bundle.standardizedFileURL.path,
                    executable: executable.standardizedFileURL.path,
                    identifier: identifier)
}

enum Failure: Error { case message(String) }

do {
    let args = CommandLine.arguments
    guard args.count == 3 || args.count == 4,
          ["snapshot", "quit", "force"].contains(args[1]) else {
        throw Failure.message("invalid internal app-control request")
    }
    let target = URL(fileURLWithPath: args[2]).standardizedFileURL
    guard target.path.hasPrefix("/"), target.lastPathComponent == "hex.app" else {
        throw Failure.message("not the configured development bundle")
    }
    let apps = NSWorkspace.shared.runningApplications.filter {
        $0.bundleURL?.standardizedFileURL == target
    }
    if args[1] == "snapshot" {
        guard args.count == 3 else { throw Failure.message("unexpected snapshot identity") }
        let data = try JSONEncoder().encode(try apps.map(identity))
        print(String(decoding: data, as: UTF8.self))
    } else {
        guard args.count == 4 else { throw Failure.message("missing exact process identity") }
        let expected = try JSONDecoder().decode(Identity.self, from: Data(args[3].utf8))
        guard apps.count == 1, let app = apps.first,
              expected.bundle == target.path,
              expected.executable == target.appendingPathComponent("Contents/MacOS/hex").path,
              expected.identifier == "org.portalsurfer.hex.dev",
              try identity(app) == expected else {
            throw Failure.message("application identity changed; no termination requested")
        }
        // Re-read immediately before the native request. No PID/name-only signal.
        guard try identity(app) == expected else {
            throw Failure.message("application identity changed; no termination requested")
        }
        let accepted = args[1] == "quit" ? app.terminate() : app.forceTerminate()
        print(accepted ? "true" : "false")
    }
} catch {
    FileHandle.standardError.write(Data("app control refused: \(error)\n".utf8))
    exit(1)
}
