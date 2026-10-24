// Internal launcher adapter. Never opens an application or targets one by name.
import AppKit
import Darwin
import Foundation

struct Identity: Codable, Equatable {
    let pid: Int32
    let started: String
    let bundle: String
    let executable: String
    let identifier: String
    let bundle_device: UInt64
    let bundle_inode: UInt64
    let executable_device: UInt64
    let executable_inode: UInt64
}

struct FilePin: Equatable {
    let device: UInt64
    let inode: UInt64
}

// No resolving symlinks or case-folding paths into trust. On a case-insensitive
// volume, different spellings qualify only when lstat proves the same object.
func checkPath(_ path: String, allowMissing: Bool = false) throws {
    guard path.hasPrefix("/") else {
        throw Failure.message("application identity path must be absolute")
    }
    // Inspect raw lexical prefixes BEFORE Foundation can standardize a URL.
    // In particular, link/../target must not hide a traversed symlink.
    var prefix = ""
    for part in path.split(separator: "/") {
        prefix += "/" + part
        var info = stat()
        let result = lstat(prefix, &info)
        if result != 0 {
            guard allowMissing, errno == ENOENT else {
                throw Failure.message("application identity path is unavailable")
            }
        } else if info.st_mode & S_IFMT == S_IFLNK {
            throw Failure.message("application identity path contains a symlink")
        }
    }
}

func filePin(_ path: String, directory: Bool) throws -> FilePin {
    try checkPath(path)
    var info = stat()
    guard lstat(path, &info) == 0,
          info.st_mode & S_IFMT == (directory ? S_IFDIR : S_IFREG),
          let device = UInt64(exactly: info.st_dev),
          let inode = UInt64(exactly: info.st_ino), inode > 0 else {
        throw Failure.message("application filesystem identity is unavailable")
    }
    return FilePin(device: device, inode: inode)
}

func targetPins(_ target: String) throws -> (FilePin, FilePin) {
    return (try filePin(target, directory: true),
            try filePin(target + "/Contents/MacOS/hex", directory: false))
}

func identity(_ app: NSRunningApplication, target: String) throws -> Identity {
    guard !app.isTerminated, let date = app.launchDate,
          let bundle = app.bundleURL, let executable = app.executableURL,
          let identifier = app.bundleIdentifier, identifier == "org.portalsurfer.hex.dev" else {
        throw Failure.message("application identity is unavailable")
    }
    let pins = try targetPins(target)
    guard try filePin(bundle.path, directory: true) == pins.0,
          try filePin(executable.path, directory: false) == pins.1 else {
        throw Failure.message("application does not match the configured filesystem target")
    }
    let after = try targetPins(target)
    guard pins.0 == after.0, pins.1 == after.1 else {
        throw Failure.message("application filesystem identity changed")
    }
    return Identity(pid: app.processIdentifier,
                    started: String(format: "%.9f", date.timeIntervalSince1970),
                    bundle: bundle.path, executable: executable.path, identifier: identifier,
                    bundle_device: pins.0.device, bundle_inode: pins.0.inode,
                    executable_device: pins.1.device, executable_inode: pins.1.inode)
}

enum Failure: Error { case message(String) }

do {
    let args = CommandLine.arguments
    guard args.count == 3 || args.count == 4,
          ["snapshot", "quit", "force"].contains(args[1]) else {
        throw Failure.message("invalid internal app-control request")
    }
    let target = args[2]
    guard target.hasPrefix("/"), target.split(separator: "/").last?.lowercased() == "hex.app" else {
        throw Failure.message("not the configured development bundle")
    }
    // A missing target may be a first install. It cannot authorize an operation.
    try checkPath(target, allowMissing: true)
    let exists = FileManager.default.fileExists(atPath: target)
    if !exists {
        guard args[1] == "snapshot", args.count == 3 else {
            throw Failure.message("configured development bundle is absent")
        }
        print("[]")
    } else {
        let pins = try targetPins(target)
        let apps = try NSWorkspace.shared.runningApplications.filter { app in
            guard let bundle = app.bundleURL else { return false }
            // A different installed bundle is not ours. Identity errors for the
            // matching target must refuse rather than silently report absence.
            guard (try? filePin(bundle.path, directory: true)) == pins.0 else { return false }
            _ = try identity(app, target: target)
            return true
        }
        if args[1] == "snapshot" {
            guard args.count == 3 else { throw Failure.message("unexpected snapshot identity") }
            let data = try JSONEncoder().encode(try apps.map { try identity($0, target: target) })
            print(String(decoding: data, as: UTF8.self))
        } else {
            guard args.count == 4 else { throw Failure.message("missing exact process identity") }
            let expected = try JSONDecoder().decode(Identity.self, from: Data(args[3].utf8))
            guard apps.count == 1, let app = apps.first,
                  expected.pid > 0, !expected.started.isEmpty,
                  expected.bundle_inode > 0, expected.executable_inode > 0,
                  try identity(app, target: target) == expected else {
                throw Failure.message("application identity changed; no termination requested")
            }
            // Re-read target and observed pins immediately before the native request.
            guard try identity(app, target: target) == expected else {
                throw Failure.message("application identity changed; no termination requested")
            }
            let accepted = args[1] == "quit" ? app.terminate() : app.forceTerminate()
            print(accepted ? "true" : "false")
        }
    }
} catch {
    FileHandle.standardError.write(Data("app control refused: \(error)\n".utf8))
    exit(1)
}
