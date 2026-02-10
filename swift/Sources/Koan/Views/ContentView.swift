import SwiftUI
import KoanRust

struct ContentView: View {
    var body: some View {
        VStack(spacing: 16) {
            Text("koan")
                .font(.system(size: 48, weight: .thin, design: .default))
            Text("v\(koanVersion())")
                .font(.system(size: 14, weight: .light, design: .monospaced))
                .foregroundStyle(.secondary)
            Text("ping: \(koan_ping())")
                .font(.system(size: 14, weight: .light, design: .monospaced))
                .foregroundStyle(.secondary)
            Text("bit-perfect or go home")
                .font(.system(size: 12, weight: .regular))
                .foregroundStyle(.tertiary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(.black)
        .foregroundStyle(.white)
    }
}

#Preview {
    ContentView()
}
