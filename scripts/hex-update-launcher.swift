// Native update helper entrypoint. It launches only the sealed Python runtime.
// No user-supplied executable, script, shell, or command arguments are accepted.
import CryptoKit
import Darwin
import Foundation

struct RuntimePolicy: Decodable {
    let version: Int
    let interpreter: String
    let interpreter_sha256: String
    let interpreter_device: UInt64
    let interpreter_inode: UInt64
}

func refuse(_ message: String) -> Never {
    FileHandle.standardError.write(Data(("update helper: " + message + "\n").utf8))
    exit(1)
}

guard CommandLine.arguments.count == 1 else { refuse("unexpected arguments") }
guard let resources = Bundle.main.resourceURL else { refuse("sealed resources unavailable") }
let policyURL = resources.appendingPathComponent("policy.json")
let entryURL = resources.appendingPathComponent("entry.py")
guard let policyData = try? Data(contentsOf: policyURL), policyData.count <= 4096,
      let policy = try? JSONDecoder().decode(RuntimePolicy.self, from: policyData),
      policy.version == 1, policy.interpreter.hasPrefix("/"),
      policy.interpreter_sha256.count == 64,
      policy.interpreter_sha256.allSatisfy({ $0.isHexDigit && !$0.isUppercase }) else {
    refuse("invalid runtime policy")
}
let interpreter = URL(fileURLWithPath: policy.interpreter)
guard interpreter.standardizedFileURL.path == interpreter.resolvingSymlinksInPath().path,
      let attrs = try? FileManager.default.attributesOfItem(atPath: interpreter.path),
      attrs[.type] as? FileAttributeType == .typeRegular,
      let size = attrs[.size] as? NSNumber, size.uint64Value <= 128 * 1024 * 1024,
      let device = attrs[.systemNumber] as? NSNumber, device.uint64Value == policy.interpreter_device,
      let inode = attrs[.systemFileNumber] as? NSNumber, inode.uint64Value == policy.interpreter_inode,
      let bytes = try? Data(contentsOf: interpreter, options: .mappedIfSafe),
      let after = try? FileManager.default.attributesOfItem(atPath: interpreter.path),
      let afterDevice = after[.systemNumber] as? NSNumber, afterDevice == device,
      let afterInode = after[.systemFileNumber] as? NSNumber, afterInode == inode,
      let afterSize = after[.size] as? NSNumber, afterSize == size else {
    refuse("configured interpreter unavailable")
}
let digest = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
guard digest == policy.interpreter_sha256 else { refuse("configured interpreter changed") }
// Replace this process instead of spawning a descendant. The desktop retains
// the same child PID and can cancel/reap the runtime even during preflight.
// The arrays contain fixed strings and a trailing null; their storage remains
// valid through execve. On failure, free every allocated C string.
let argumentStrings = [interpreter.path, "-I", "-B", entryURL.path]
let environmentStrings = ["PATH=/usr/bin:/bin:/usr/sbin:/sbin", "LANG=C", "LC_ALL=C"]
var arguments = argumentStrings.map { strdup($0) }
var environment = environmentStrings.map { strdup($0) }
guard arguments.allSatisfy({ $0 != nil }), environment.allSatisfy({ $0 != nil }) else {
    for value in arguments + environment { free(value) }
    refuse("runtime arguments unavailable")
}
arguments.append(nil)
environment.append(nil)
let result = interpreter.path.withCString { path in
    arguments.withUnsafeMutableBufferPointer { argv in
        environment.withUnsafeMutableBufferPointer { envp in
            guard let argvBase = argv.baseAddress, let envpBase = envp.baseAddress else { return Int32(-1) }
            return execve(path, argvBase, envpBase)
        }
    }
}
for value in arguments + environment { free(value) }
_ = result
refuse("runtime failed to start")
