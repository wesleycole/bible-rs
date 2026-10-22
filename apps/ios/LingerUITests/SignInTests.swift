import XCTest

/// Signs in through Clerk's own sheet on the development instance as its existing test user, whose
/// email code is 424242 without any mail sent (server/scripts/clerk-test-token.ts signs in the
/// same user), and lands in the reader. A simulator that is already signed in signs out first,
/// through Settings.
final class SignInTests: XCTestCase {
    func testSigningInOpensTheReader() {
        let app = XCUIApplication()
        app.launch()
        app.signOutIfSignedIn()
        app.signIn()
        for tab in ["Bible", "Library", "Settings", "Ask"] {
            XCTAssertTrue(app.tabBars.buttons[tab].exists, "no \(tab) tab")
        }
    }
}

extension XCUIApplication {
    /// The jump field, which shows once signed in.
    var reader: XCUIElement { descendants(matching: .any)["jump-field"].firstMatch }

    /// Leaves the app signed in on the reader, signing in as Clerk's test user if it isn't.
    func signIn() {
        let start = buttons["Sign in"]
        waitForLaunch()
        guard start.exists else { return }
        start.tap()

        // Until it has focus, an empty field shows only as its label, so match the identifier on any
        // element; once focused it is a text field, holding the address Clerk remembers, if any.
        let address = "amp+clerk_test@example.com"
        let email = descendants(matching: .any)["clerk.auth.start.identifier"].firstMatch
        XCTAssertTrue(email.waitForExistence(timeout: 15), "Clerk's sign-in sheet didn't open")
        email.tap()
        let field = textFields["clerk.auth.start.identifier"]
        XCTAssertTrue(field.waitForExistence(timeout: 5), "the email field didn't take focus")
        let remembered = (field.value as? String) ?? ""
        if remembered != address {
            typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: remembered.count) + address)
        }
        buttons["clerk.auth.start.continue"].tap()

        // The development instance offers this user's password first; take the email code.
        let other = buttons["clerk.auth.signIn.useAnotherMethod"]
        XCTAssertTrue(other.waitForExistence(timeout: 15))
        other.tap()
        buttons["clerk.auth.signIn.alternativeMethod.email_code"].tap()

        XCTAssertTrue(staticTexts["Check your email"].waitForExistence(timeout: 15))
        // The code field has focus.
        typeText("424242")

        // The library consent sheet, unless this device already answered it.
        let consent = buttons["Turn On Sync"]
        if consent.waitForExistence(timeout: 10) { consent.tap() }
        XCTAssertTrue(reader.waitForExistence(timeout: 20), "the reader didn't appear")
    }

    /// Waits until the app shows either the sign-in screen or the reader.
    private func waitForLaunch() {
        let shown = NSPredicate { _, _ in self.buttons["Sign in"].exists || self.reader.exists }
        let wait = XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: shown, object: nil)], timeout: 20)
        XCTAssertEqual(wait, .completed, "neither sign-in nor the reader appeared")
    }

    /// Signs out through the Settings tab if the app opened signed in.
    func signOutIfSignedIn() {
        let start = buttons["Sign in"]
        waitForLaunch()
        guard !start.exists else { return }
        tabBars.buttons["Settings"].tap()
        let signOut = buttons["Sign out"]
        XCTAssertTrue(signOut.waitForExistence(timeout: 10))
        signOut.tap()
        XCTAssertTrue(start.waitForExistence(timeout: 30), "signing out didn't return to the sign-in screen")
    }
}
