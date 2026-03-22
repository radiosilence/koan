import SwiftUI

struct ServerSettingsView: View {
    @Environment(ServerConfig.self) private var config
    @State private var testResult: String?
    @State private var testSuccess = false
    @State private var isTesting = false

    var body: some View {
        @Bindable var config = config
        Form {
            Section("Server") {
                TextField("Host", text: $config.host)
                    .textContentType(.URL)
                    .autocorrectionDisabled()
                    #if os(iOS)
                    .textInputAutocapitalization(.never)
                    #endif

                HStack {
                    Text("Port")
                    Spacer()
                    TextField("Port", value: $config.port, format: .number)
                        .multilineTextAlignment(.trailing)
                        .frame(width: 80)
                        #if os(iOS)
                        .keyboardType(.numberPad)
                        #endif
                }

                LabeledContent("URL", value: config.baseURL.absoluteString)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section {
                Button {
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

                if let testResult {
                    HStack {
                        Image(systemName: testSuccess ? "checkmark.circle.fill" : "xmark.circle.fill")
                            .foregroundStyle(testSuccess ? .green : .red)
                        Text(testResult)
                            .font(.caption)
                    }
                }
            }
        }
        .navigationTitle("Settings")
    }

    private func testConnection() async {
        isTesting = true
        defer { isTesting = false }

        do {
            let response: LibraryStatsResponse = try await config.client.execute(query: GQL.libraryStats)
            let stats = response.libraryStats
            testResult = "Connected. \(stats.totalTracks) tracks, \(stats.totalAlbums) albums, \(stats.totalArtists) artists."
            testSuccess = true
        } catch {
            testResult = error.localizedDescription
            testSuccess = false
        }
    }
}
