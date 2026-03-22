import SwiftUI

struct ServerSettingsView: View {
    @State private var urlText: String = ServerConfig.shared.serverURL
    @State private var testResult: TestResult?
    @State private var isTesting = false

    private var client: GraphQLClient { .shared }

    enum TestResult {
        case success(LibraryStats)
        case failure(String)
    }

    var body: some View {
        Form {
            Section("Server") {
                TextField("Server URL", text: $urlText)
                    .textContentType(.URL)
                    #if os(iOS)
                    .textInputAutocapitalization(.never)
                    #endif
                    .autocorrectionDisabled()
                    .onSubmit { saveURL() }

                Button {
                    saveURL()
                    Task { await testConnection() }
                } label: {
                    HStack {
                        Text("Test Connection")
                        Spacer()
                        if isTesting {
                            ProgressView()
                        }
                    }
                }
                .disabled(isTesting)
            }

            if let testResult {
                Section("Status") {
                    switch testResult {
                    case .success(let stats):
                        Label("Connected", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.green)
                        LabeledContent("Artists", value: "\(stats.totalArtists)")
                        LabeledContent("Albums", value: "\(stats.totalAlbums)")
                        LabeledContent("Tracks", value: "\(stats.totalTracks)")
                    case .failure(let message):
                        Label("Failed", systemImage: "xmark.circle.fill")
                            .foregroundStyle(.red)
                        Text(message)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
        .navigationTitle("Settings")
    }

    private func saveURL() {
        ServerConfig.shared.serverURL = urlText
    }

    private func testConnection() async {
        isTesting = true
        defer { isTesting = false }

        do {
            let response: LibraryStatsResponse = try await client.execute(query: GQL.libraryStats)
            testResult = .success(response.libraryStats)
        } catch {
            testResult = .failure(error.localizedDescription)
        }
    }
}
