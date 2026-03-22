import SwiftUI

struct ServerSettingsView: View {
    @AppStorage("serverURL") private var serverURLString = "http://localhost:3000"
    @State private var editedURL = ""
    @State private var testResult: TestResult?

    enum TestResult {
        case success
        case failure(String)
    }

    var body: some View {
        Form {
            Section("Server") {
                TextField("Server URL", text: $editedURL)
                    .autocorrectionDisabled()
                    #if os(iOS)
                    .textContentType(.URL)
                    .textInputAutocapitalization(.never)
                    .keyboardType(.URL)
                    #endif

                Button("Test Connection") {
                    testConnection()
                }
            }

            if let result = testResult {
                Section {
                    switch result {
                    case .success:
                        Label("Connected", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.green)
                    case .failure(let msg):
                        Label(msg, systemImage: "xmark.circle.fill")
                            .foregroundStyle(.red)
                    }
                }
            }

            Section {
                Button("Save") {
                    serverURLString = editedURL
                }
                .disabled(editedURL == serverURLString)
            }
        }
        .navigationTitle("Server")
        .onAppear {
            editedURL = serverURLString
        }
    }

    private func testConnection() {
        guard let url = URL(string: editedURL) else {
            testResult = .failure("Invalid URL")
            return
        }

        testResult = nil
        let client = GraphQLClient(baseURL: url)

        Task {
            do {
                _ = try await client.fetchNowPlaying()
                await MainActor.run { testResult = .success }
            } catch {
                await MainActor.run { testResult = .failure(error.localizedDescription) }
            }
        }
    }
}
