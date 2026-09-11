import Foundation

struct Host: Codable, Identifiable, Equatable {
    enum Transport: String, Codable, CaseIterable, Identifiable {
        case ssh, mosh
        var id: String { rawValue }
        var label: String { self == .ssh ? "SSH" : "mosh" }
    }

    var id = UUID()
    var name = ""
    var address = ""
    var port = 22
    var user = ""
    var keyID: UUID?
    var transport: Transport = .ssh
    var agentForwarding = false
    var sshFlags = ""
    var moshFlags = ""
    var remoteCommand = ""

    var title: String { name.isEmpty ? "\(user)@\(address)" : name }
    var subtitle: String { "\(user)@\(address):\(port)" }
}

struct Key: Codable, Identifiable, Equatable {
    var id = UUID()
    var name = ""
    var publicKey = ""
}
