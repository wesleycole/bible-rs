import SwiftUI

@main
struct LingerApp: App {
    @State private var model = ReaderModel()

    var body: some Scene {
        WindowGroup {
            ReaderView(model: model)
        }
    }
}
