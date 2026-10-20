import LingerCore
import SwiftUI

/// The library consent sheet: shown once, signed in, before the library syncs. Its words are
/// core's, the same as the desktop's.
struct ConsentSheet: View {
    let library: Library
    private let text = libraryConsentText()

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Text(text.title).font(.title2.bold())
            ForEach(text.points, id: \.self) { point in
                Label(point, systemImage: "circle.fill")
                    .labelStyle(Bullet())
            }
            Spacer()
            Button("Turn On Sync") { library.answerConsent(true) }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .frame(maxWidth: .infinity)
            Button("Keep on This iPhone") { library.answerConsent(false) }
                .frame(maxWidth: .infinity)
        }
        .padding(28)
        .interactiveDismissDisabled()
    }

    private struct Bullet: LabelStyle {
        func makeBody(configuration: Configuration) -> some View {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                configuration.icon.font(.system(size: 6)).foregroundStyle(.secondary)
                configuration.title.foregroundStyle(.secondary)
            }
        }
    }
}
