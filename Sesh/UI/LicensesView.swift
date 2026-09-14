import SwiftUI

struct Licenses: Decodable {
    struct Component: Decodable, Identifiable {
        let name: String, version: String, license: String, texts: [Int]
        var id: String { name + version }
    }
    let texts: [String]
    let components: [Component]
    let gpl: Int

    static let bundled: Licenses = {
        guard let url = Bundle.main.url(forResource: "licenses", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let parsed = try? JSONDecoder().decode(Licenses.self, from: data)
        else { return Licenses(texts: [], components: [], gpl: 0) }
        return parsed
    }()
}

private let sourceURLs = ["https://code.pecheny.me/pecheny/sesh",
                          "https://code.pecheny.me/pecheny/rmosh"]

struct LicensesView: View {
    @Environment(\.dismiss) private var dismiss
    private let licenses = Licenses.bundled

    var body: some View {
        NavigationStack {
            List {
                Section("Sesh") {
                    Text("""
                    Sesh is free software: a build links mosh through rmosh, so the app as \
                    a whole is under the GNU General Public License, version 3 or later, \
                    and comes with no warranty. Sesh's own source is MIT.

                    You are entitled to the corresponding source of this build. It lives at:
                    """).font(.ui(13))
                    ForEach(sourceURLs, id: \.self) { url in
                        Link(url, destination: URL(string: url)!).font(.ui(13))
                    }
                    NavigationLink("GNU General Public License v3") {
                        TextScreen(title: "GPL v3",
                                   body: licenses.texts[safe: licenses.gpl] ?? "")
                    }
                    .font(.ui(14))
                }
                Section("What the app links or bundles") {
                    ForEach(licenses.components) { component in
                        NavigationLink {
                            TextScreen(title: component.name,
                                       body: component.texts
                                           .map { licenses.texts[$0] }
                                           .joined(separator: "\n\n\n"))
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(component.name).font(.ui(14))
                                Text([component.version, component.license]
                                    .filter { !$0.isEmpty }.joined(separator: " — "))
                                    .font(.ui(11)).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
            .navigationTitle("Licences")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
    }
}

private extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}

private struct TextScreen: View {
    let title: String
    let body_: String
    init(title: String, body: String) { self.title = title; self.body_ = body }

    var body: some View {
        ScrollView {
            Text(body_).font(.system(size: 11, design: .monospaced))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
    }
}
